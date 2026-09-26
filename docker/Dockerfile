# syntax=docker/dockerfile:1

# --- build the rust binaries ---
FROM rust:1-bookworm AS rust-builder
WORKDIR /app
COPY Cargo.toml Cargo.lock* rust-toolchain.toml ./
COPY crates ./crates
# ui/ is a workspace member (rolter-ui marker crate for release-plz changelog
# tracking); copy its manifest + stub lib so cargo can resolve the workspace
COPY ui/Cargo.toml ui/changelog.rs ./ui/
# optional workspace features (empty by default, so the shipping image is
# unchanged). set CARGO_FEATURES=postgres to build the control plane with its
# postgres-backed store, CRUD API and migrations (the e2e harness does this).
ARG CARGO_FEATURES=""
# the crate registry, the git checkouts and the target directory are buildkit
# cache mounts, so a rebuild reuses downloaded crates and compiled dependency
# artifacts instead of starting from nothing. without them a one-line source
# change costs a full cold build — measured at ~90 min on a laptop vm, of which
# ~46 min was re-downloading crates that were already on disk.
#
# a cache mount is not part of the resulting layer, so the binaries have to be
# copied somewhere persistent inside the same RUN; the runtime stage takes them
# from /out rather than from target/.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    cargo build --release --workspace ${CARGO_FEATURES:+--features "$CARGO_FEATURES"} \
    && mkdir -p /out \
    && cp target/release/rolter target/release/rolter-gateway target/release/rolter-control /out/

# --- build the ui ---
FROM oven/bun:1 AS ui-builder
WORKDIR /ui
# bun.lock (text) — the glob used to say bun.lockb (bun's old binary name), which
# matched nothing, so the image resolved dependencies unpinned on every build and
# broke as soon as a transitive version drifted. copy the real lockfile and honour
# it, so the image is reproducible and matches CI
COPY ui/package.json ui/bun.lock ./
RUN bun install --frozen-lockfile
COPY ui/ ./
# the dashboard version is read from the Cargo workspace manifest at build
# time, so that one version stays the only one anybody bumps (#953). without
# this the vite build fails loudly rather than shipping a wrong version
COPY Cargo.toml /Cargo.toml
RUN bun run build

# --- runtime ---
# distroless: no shell, no package manager, and crucially no `perl-base`, which
# on debian carries unfixed CVEs (CVE-2026-12087 et al) that a base bump cannot
# resolve. the binaries link only glibc (libc/libm/libgcc), and reqwest uses
# rustls with bundled webpki roots, so no system openssl or ca-certificates is
# needed. runs as the built-in nonroot user (uid 65532). pinned by index digest
# (multi-arch: amd64 + arm64).
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:ce0d66bc0f64aae46e6a03add867b07f42cc7b8799c949c2e898057b7f75a151 AS runtime
WORKDIR /app
COPY --from=rust-builder /out/rolter /usr/local/bin/rolter
COPY --from=rust-builder /out/rolter-gateway /usr/local/bin/rolter-gateway
COPY --from=rust-builder /out/rolter-control /usr/local/bin/rolter-control
COPY --from=ui-builder /ui/dist /app/ui/dist
COPY rolter.example.toml /app/rolter.toml
# bind every interface: a published port (`docker run -p 4000:4000`) forwards to
# the container's own network interface, never to its loopback, so a loopback
# bind answers nothing from the host (#1891). ROLTER_HOST covers `easy-up` (the
# default command) and `rolter-gateway`; ROLTER_CONTROL_HOST covers a bare
# `rolter-control`. this does not reopen #970: with no ROLTER_ADMIN_TOKEN both
# `easy-up` and the control plane still refuse a non-loopback bind unless
# ROLTER_ALLOW_OPEN_MODE=1 acknowledges it
ENV ROLTER_CONFIG=/app/rolter.toml \
    ROLTER_UI_DIR=/app/ui/dist \
    ROLTER_HOST=0.0.0.0 \
    ROLTER_CONTROL_HOST=0.0.0.0 \
    RUST_LOG=info
EXPOSE 4000 4001
CMD ["/usr/local/bin/rolter", "easy-up", "--config", "/app/rolter.toml", "--ui-dir", "/app/ui/dist"]
