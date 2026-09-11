# Commit conventions

rolter uses [Conventional Commits](https://www.conventionalcommits.org) for commit messages **and** PR titles. CI checks PR titles; the `conventional-pre-commit` hook managed by `prek` checks local messages.

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

## What `BREAKING CHANGE:` means after 1.0

Before 1.0 the trailer meant "something moved", a Rust API included. From 1.0 it
means one thing, decided in
[ADR-0032](../adr/2026-09-09-one-point-oh-compatibility-guarantees.md): **a
change to a promised surface that ships without a completed deprecation
window.** All crates share one version, so a single trailer takes the whole
product — binaries, wheel and Docker tag — to the next major. It is expensive on
purpose, and it needs a written migration note in the release.

It does **not** apply to:

- a removal that completed its deprecation window (two minor releases and 90
  days for `/api/v1/*` and for config keys). The window *was* the notice; the
  removal ships as an ordinary `feat`/`refactor` whose release notes name it;
- a Rust API change in any crate — they are internal and carry no promise;
- rolter tracking a change OpenAI or Anthropic made to their own dialect;
- a subsystem carrying the `experimental`
  [stability marker](stability-markers.md), which is the documented exemption.

The rule behind it: if the change *could* have been deprecated first, it must
be, and then it is not breaking.

## Issues & PRs

- Link issues from the body/footer: `Closes #123`, `Refs #123`.
- PR title must be a single valid Conventional Commit line (enforced by CI via `amannn/action-semantic-pull-request`).
- Squash-merge so the PR title becomes the commit on `master`; keeps history releasable and changelog-friendly.

## Tooling

- `.config/commitlint.config.mjs` — rules (types, scopes, lowercase subject, 72-char header).
- `prek.toml` — fast commit-time hygiene, secret scanning, formatting/linting, commit-message validation, and pre-push test/security gates.
- Install all configured hook stages with `prek install --prepare-hooks`. The configuration installs `pre-commit`, `commit-msg`, and `pre-push` shims.
- Run commit-time checks manually with `prek run --all-files`.
- Run the push gate manually with `prek run --all-files --hook-stage pre-push`.

The commit stage uses prek's built-in checks plus pinned Gitleaks and
Conventional Commit hooks. Project-specific checks require `actionlint`,
`taplo`, and `typos` on `PATH`. The push stage also requires `cargo-deny` and
Bun when UI files are part of the push. Install `cargo-nextest` for CI-equivalent
test execution; the hook falls back to `cargo test` when it is unavailable.
