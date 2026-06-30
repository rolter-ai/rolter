# Commit conventions

rolter uses [Conventional Commits](https://www.conventionalcommits.org) for commit messages **and** PR titles. CI checks PR titles; the `conventional-pre-commit` hook checks local messages.

## Format

```
<type>(<scope>): <subject>

<body>

<footer>
```

- **type** (required): `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`, `revert`
- **scope** (recommended): `gateway`, `balancer`, `proxy`, `core`, `store`, `auth`, `control`, `ui`, `docs`, `infra`, `ci`, `deps`, `release`
- **subject**: imperative, lowercase, ≤ 72 chars, no trailing period
- **breaking change**: add `!` after the scope and a `BREAKING CHANGE:` footer

## Examples

```
feat(balancer): add precise kv-event cache-aware scorer
fix(gateway): stream anthropic sse without buffering
perf(proxy): reuse pooled client per egress proxy
docs(architecture): document reload-free config propagation
refactor(core)!: rename ModelRoute.targets to upstreams

BREAKING CHANGE: config field `targets` is now `upstreams`.
```

## Issues & PRs

- Link issues from the body/footer: `Closes #123`, `Refs #123`.
- PR title must be a single valid Conventional Commit line (enforced by CI via `amannn/action-semantic-pull-request`).
- Squash-merge so the PR title becomes the commit on `master`; keeps history releasable and changelog-friendly.

## Tooling

- `commitlint.config.mjs` — rules (types, scopes, lowercase subject, 72-char header).
- `.pre-commit-config.yaml` — `conventional-pre-commit` (commit-msg) + `cargo fmt`/`cargo clippy`.
- Install hooks: `prek install` (or `pre-commit install && pre-commit install --hook-type commit-msg`).
