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
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=rust-builder /app/target/release/rolter-gateway /usr/local/bin/rolter-gateway
COPY --from=rust-builder /app/target/release/rolter-control /usr/local/bin/rolter-control
COPY --from=ui-builder /ui/dist /app/ui/dist
COPY rolter.example.toml /app/rolter.toml
ENV ROLTER_CONFIG=/app/rolter.toml \
    ROLTER_UI_DIR=/app/ui/dist \
    RUST_LOG=info
EXPOSE 4000 4001
CMD ["rolter-gateway", "--config", "/app/rolter.toml"]
