# syntax=docker/dockerfile:1

# --- build the rust binaries ---
FROM rust:1-bookworm AS rust-builder
WORKDIR /app
COPY Cargo.toml Cargo.lock* rust-toolchain.toml ./
COPY crates ./crates
RUN cargo build --release --workspace

# --- build the ui ---
FROM oven/bun:1 AS ui-builder
WORKDIR /ui
COPY ui/package.json ui/bun.lockb* ./
RUN bun install
COPY ui/ ./
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
COPY --from=rust-builder /app/target/release/rolter-gateway /usr/local/bin/rolter-gateway
COPY --from=rust-builder /app/target/release/rolter-control /usr/local/bin/rolter-control
COPY --from=ui-builder /ui/dist /app/ui/dist
COPY rolter.example.toml /app/rolter.toml
ENV ROLTER_CONFIG=/app/rolter.toml \
    ROLTER_UI_DIR=/app/ui/dist \
    RUST_LOG=info
EXPOSE 4000 4001
CMD ["/usr/local/bin/rolter-gateway", "--config", "/app/rolter.toml"]
