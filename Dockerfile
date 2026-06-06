# ── Stage 1: Builder ──────────────────────────────────────────────────────────
FROM rust:slim-bookworm AS builder

# System deps for sqlx / openssl / native-tls
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Cache dependencies: copy manifests first, build a dummy lib, then replace
COPY Cargo.toml ./
# Create a stub main so `cargo build` succeeds for dep caching
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs
RUN cargo build --release 2>/dev/null || true

# Now copy the real source and rebuild (only app code recompiles)
COPY src ./src
COPY migrations ./migrations

# Touch main.rs to force recompile of the binary crate
RUN touch src/main.rs && cargo build --release

# ── Stage 2: Runtime ──────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    sqlite3 \
    && rm -rf /var/lib/apt/lists/*

# Non-root user
RUN useradd -ms /bin/bash appuser

# Data directory for SQLite
RUN mkdir -p /data && chown appuser:appuser /data

# Config directory
RUN mkdir -p /config && chown appuser:appuser /config

WORKDIR /app
COPY --from=builder /build/target/release/mplus-tracker /app/mplus-tracker

USER appuser

EXPOSE 8080

ENV CONFIG_PATH=/config/config.toml \
    DATABASE_PATH=/data/mplus.sqlite \
    RUST_LOG=info

ENTRYPOINT ["/app/mplus-tracker"]
