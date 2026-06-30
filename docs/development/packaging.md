# Packaging & distribution

rolter ships three ways.

## cargo

```bash
cargo install --path crates/rolter-gateway
cargo install --path crates/rolter-control
```

## uv (PyPI wheel via maturin)

The wheel bundles the compiled Rust binary so Python users can install the CLI with `uv`. `pyproject.toml` uses the maturin backend (`bindings = "bin"`, `manifest-path = crates/rolter-gateway/Cargo.toml`).

```bash
uv tool install maturin       # one-time
uvx maturin build --release   # build a wheel into target/wheels/
uv tool install rolter        # once published to PyPI
```

> Note: the wheel currently exposes `rolter-gateway`. A unified `rolter` launcher that dispatches `gateway`/`control` subcommands (so one wheel ships both) is tracked in `TODO.md`.

## Docker

Multi-stage `Dockerfile` builds the Rust binaries and the Bun-built UI, then assembles a slim runtime:

```bash
docker build -t rolter:dev .
docker compose up -d          # full stack with postgres/redis/clickhouse
```

## Releasing (roadmap)

- Tag-driven CI: build manylinux/macos wheels (cibuildwheel/maturin-action), publish to PyPI; publish crates to crates.io; push multi-arch images to GHCR.
- Conventional Commits enable automated changelog/version bumps (e.g. release-please/semantic-release).
