# Contributing to rolter

Thanks for helping build rolter! This is a short pointer; the full guide lives
in [`docs/development/contributing.md`](docs/development/contributing.md).

## Quick start

1. Branch from `master`: `feat/<scope>-<short>` or `fix/<scope>-<short>`.
2. Make focused changes; add unit tests next to the code.
3. Run the checks:
   ```bash
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
4. Use [Conventional Commits](docs/development/commit-conventions.md) for commit
   messages and the PR title.
5. Link issues with `Closes #N` / `Refs #N`.
6. Open a PR, fill in the template, and make sure CI is green.

## More

- Full contributing guide: [`docs/development/contributing.md`](docs/development/contributing.md)
- Dev setup: [`docs/development/setup.md`](docs/development/setup.md)
- Testing: [`docs/development/testing.md`](docs/development/testing.md)
- Architecture: [`docs/architecture/overview.md`](docs/architecture/overview.md)
- Code of Conduct: [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md)
- Security policy: [`SECURITY.md`](SECURITY.md)

By contributing, you agree your contributions are licensed under the project's
[Apache-2.0](LICENSE) license.
