# syntax=docker/dockerfile:1

# ── Builder ──────────────────────────────────────────────────────────────────
FROM rust:1-slim-trixie AS builder
WORKDIR /app

# fastembed/ONNX links C++ support. WebAuthn attestation verification uses the
# platform OpenSSL library; outbound application HTTP remains rustls-only.
# `make` builds vendored jemalloc (tikv-jemalloc-sys) for the server/worker
# binaries' global allocator (see docs/FINDINGS.md).
RUN apt-get update \
 && apt-get install -y --no-install-recommends g++ make pkg-config libssl-dev \
 && rm -rf /var/lib/apt/lists/*

# Copy build inputs after system dependencies for better layer caching.
COPY Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml ./
COPY crates ./crates
COPY bins ./bins

# Compile release binaries. Rusty Feed uses runtime SQLx queries (validated by the
# integration tests against a real Postgres), so no database or offline metadata
# is needed at build time.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --bin rusty-feed-server --bin rusty-feed-worker --bin rusty-feed-cli \
 && mkdir -p /out \
 && cp target/release/rusty-feed-server target/release/rusty-feed-worker target/release/rusty-feed-cli /out/

# ── Runtime ──────────────────────────────────────────────────────────────────
FROM debian:trixie-slim AS runtime
# ca-certificates for outbound TLS; libstdc++ for ONNX; libssl for WebAuthn
# attestation verification; tini for signal handling.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libstdc++6 libssl3t64 tini \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --create-home --uid 10001 rusty-feed \
 && mkdir -p /var/lib/rusty-feed/models \
 && chown -R rusty-feed:rusty-feed /var/lib/rusty-feed

COPY --from=builder /out/rusty-feed-server /out/rusty-feed-worker /out/rusty-feed-cli /usr/local/bin/
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

USER rusty-feed
ENV MODEL_CACHE_DIR=/var/lib/rusty-feed/models \
    BIND_ADDR=0.0.0.0:8080
EXPOSE 8080

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/entrypoint.sh"]
CMD ["rusty-feed-server"]
