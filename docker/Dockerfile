# syntax=docker/dockerfile:1

# ── Builder ──────────────────────────────────────────────────────────────────
FROM rust:1-slim-bookworm AS builder
WORKDIR /app

# Build dependencies first for better layer caching.
# (rustls is used for TLS, so no OpenSSL/pkg-config needed.)
COPY Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml ./
COPY crates ./crates
COPY bins ./bins

# Compile release binaries. SQLX_OFFLINE ensures compile-time query checks use
# the committed .sqlx/ metadata rather than a live database (added in M2).
ENV SQLX_OFFLINE=true
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --bin rustss-server --bin rustss-worker --bin rustss-cli \
 && mkdir -p /out \
 && cp target/release/rustss-server target/release/rustss-worker target/release/rustss-cli /out/

# ── Runtime ──────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime
# ca-certificates for outbound TLS; tini for proper signal handling / reaping.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates tini \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --create-home --uid 10001 rustss \
 && mkdir -p /var/lib/rustss/models \
 && chown -R rustss:rustss /var/lib/rustss

COPY --from=builder /out/rustss-server /out/rustss-worker /out/rustss-cli /usr/local/bin/
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

USER rustss
ENV MODEL_CACHE_DIR=/var/lib/rustss/models \
    BIND_ADDR=0.0.0.0:8080
EXPOSE 8080

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/entrypoint.sh"]
CMD ["rustss-server"]
