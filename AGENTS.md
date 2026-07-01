# AGENTS.md

Guidance for humans and AI agents working in this repository. `CLAUDE.md` is a symlink to this file.

## Project

rolter is a high-performance OpenAI/Anthropic-compatible AI gateway and load balancer. The backend is Rust (a Cargo workspace with two binaries over shared crates); the dashboard is a Vite + React + shadcn/ui SPA served as static assets by the control plane.

## Commands

- `cargo build --workspace` — build everything
- `cargo test --workspace` — run unit tests
- `cargo fmt --all` — format (run before committing)
- `cargo clippy --workspace --all-targets -- -D warnings` — lint (must be clean)
- `cargo run -p rolter-gateway -- --config rolter.toml` — run the data plane
- `cargo run -p rolter-control` — run the control plane + UI host (add `--database-url`/`ROLTER_DATABASE_URL` for the postgres-backed store, CRUD API and `/internal/snapshot`)
- `cargo run -p rolter-store --features postgres --bin rolter-seed -- --import rolter.example.toml` — idempotent DB bootstrap (org/team/project, optional admin user, providers/routes)
- `cd ui && bun install` then `bun run dev` / `bun run build` — UI deps, dev server, production build
- `docker compose up -d` — bring up Postgres, Redis, ClickHouse and rolter

## Code standards

- Rust 2021, `rustfmt` defaults, `clippy` clean with `-D warnings`.
- Prefer `thiserror` for library errors and `anyhow` only in binaries.
- Keep the data-plane hot path allocation-light; do not block on locks (use `arc-swap` for config reads).
- Avoid `unwrap()`/`expect()` on request paths; map errors to OpenAI-style JSON.
- Code comments start lowercase with no trailing punctuation; `///` doc comments use normal prose.
- New balancing strategies implement `rolter_balancer::LoadBalancer` and are wired into `build()`.
- New storage backends implement the `rolter_store` traits behind a cargo feature.
- The gateway ships a built-in `fake-llm` model (deterministic lorem ipsum, no upstream or config needed) on `/v1/chat/completions` and `/v1/messages`, non-streaming and SSE. Use it for smoke tests and local dev without secrets; a configured route named `fake-llm` shadows the builtin.

## Commit & PR conventions

This repo uses **Conventional Commits** for commit messages and PR titles. Format:

```
<type>(<scope>): <subject>
```

- **types**: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`, `revert`
- **scopes**: `gateway`, `balancer`, `proxy`, `core`, `store`, `auth`, `control`, `ui`, `docs`, `infra`, `ci`, `deps`
- subject is imperative, lowercase, ≤ 72 chars, no trailing period
- breaking changes: add `!` after the scope (`feat(core)!: ...`) and a `BREAKING CHANGE:` footer

Examples:

```
feat(balancer): add precise kv-event cache-aware scorer
fix(gateway): stream anthropic sse without buffering
docs(architecture): document reload-free config propagation
```

- Link Linear issues in the body/footer with `Closes ROL-123` / `Refs ROL-123` (GitHub issues: `Closes #123`).
- PR titles must be a single valid Conventional Commit line (CI checks this); append the Linear key in brackets, e.g. `feat(gateway): built-in fake-llm default model [ROL-98]`.
- Branch names follow `<type>/<ISSUE-KEY>-<short-description>` with the same Conventional Commit types, e.g. `fix/ROL-94-models-auth`. Never use a person or agent name as the prefix.
- Prefer stacked branches/PRs (GitButler) for dependent work; keep each PR one logical change.
- Keep PRs focused; update `docs/` and `TODO.md` when behavior changes.
- Include the co-author trailer on agent commits: `Co-Authored-By: Oz <oz-agent@warp.dev>`.

Commit hygiene is enforced by `commitlint` (PR titles) and the `conventional-pre-commit` hook in `.pre-commit-config.yaml`.

## Testing & quality

- Add unit tests next to the code (`#[cfg(test)] mod tests`).
- Run `cargo test` and `cargo clippy` before committing.
- Never commit secrets; provider keys come from env vars or the encrypted store.
