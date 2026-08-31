# syntax=docker/dockerfile:1

# ── Builder ──────────────────────────────────────────────────────────────────
FROM rust:1-slim-trixie AS builder
WORKDIR /app

# fastembed/ONNX links C++ support. WebAuthn attestation verification uses the
# platform OpenSSL library; outbound application HTTP remains rustls-only.
RUN apt-get update \
 && apt-get install -y --no-install-recommends g++ pkg-config libssl-dev \
 && rm -rf /var/lib/apt/lists/*

# Copy build inputs after system dependencies for better layer caching.
COPY Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml ./
COPY crates ./crates
COPY bins ./bins

# Compile release binaries. RustSS uses runtime SQLx queries (validated by the
# integration tests against a real Postgres), so no database or offline metadata
# is needed at build time.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --bin rustss-server --bin rustss-worker --bin rustss-cli \
 && mkdir -p /out \
 && cp target/release/rustss-server target/release/rustss-worker target/release/rustss-cli /out/

# ── Runtime ──────────────────────────────────────────────────────────────────
FROM debian:trixie-slim AS runtime
# ca-certificates for outbound TLS; libstdc++ for ONNX; libssl for WebAuthn
# attestation verification; tini for signal handling.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libstdc++6 libssl3t64 tini \
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
