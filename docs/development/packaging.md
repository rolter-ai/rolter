# Packaging & distribution

rolter ships three ways.

The unified `rolter` binary dispatches to both planes via subcommands:

```bash
rolter gateway --config rolter.toml     # data plane
rolter control --database-url postgres://…   # control plane + UI host
```

The standalone `rolter-gateway` / `rolter-control` binaries remain available.

## cargo

```bash
cargo install rolter            # unified launcher (from crates.io)
# or from source:
cargo install --path crates/rolter
```

## uv (PyPI wheel via maturin)

The wheel bundles the compiled `rolter` launcher so Python users can install the CLI with `uv`. `pyproject.toml` uses the maturin backend (`bindings = "bin"`, `manifest-path = crates/rolter/Cargo.toml`).

```bash
uv tool install maturin       # one-time
uvx maturin build --release   # build a wheel into target/wheels/
uv tool install rolter        # once published to PyPI
```

## Docker

Multi-stage `docker/Dockerfile` builds the Rust binaries and the Bun-built UI, then assembles a slim runtime:

```bash
docker build -f docker/Dockerfile -t rolter:dev .
docker compose -f docker/docker-compose.yml up -d          # full stack with postgres/redis/clickhouse
```

## Releasing (roadmap)

- Tag-driven CI: build manylinux/macos wheels (cibuildwheel/maturin-action), publish to PyPI; publish crates to crates.io; push multi-arch images to GHCR.
- Conventional Commits enable automated changelog/version bumps (e.g. release-please/semantic-release).
