# Contributing

Thanks for helping build rolter.

## Workflow

1. Branch from `master`: `feat/<scope>-<short>` or `fix/<scope>-<short>`.
2. Make focused changes; add unit tests next to the code.
3. Run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
4. Use [Conventional Commits](commit-conventions.md) for messages and the PR title.
5. Link issues with `Closes #N` / `Refs #N`.
6. Open a PR; fill in the template; CI must be green.

## Code standards

- Rust 2021; `rustfmt` defaults; clippy clean with `-D warnings`.
- `thiserror` for library errors, `anyhow` in binaries.
- Keep the data-plane hot path allocation-light; never block on locks (use `arc-swap`).
- Code comments start lowercase, no trailing punctuation; `///` doc comments use normal prose.
- New balancing strategy → implement `rolter_balancer::LoadBalancer` + wire into `build()`.
- New storage backend → implement `rolter_store` traits behind a cargo feature.

## Agent commits

Automated contributions include the trailer:

```
Co-Authored-By: Oz <oz-agent@warp.dev>
```

## Don't

- Don't commit secrets (use env / the encrypted store).
- Don't force-push `master`; force-push is fine on your own feature branches.
- Don't `--amend` after pushing shared history.
