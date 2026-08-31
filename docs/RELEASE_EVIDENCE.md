# v1 release evidence

Evidence recorded on **2026-08-30–31**. This is a reproducible acceptance record,
not a promise that every unrelated Miniflux v2/backlog item ships in RustSS v1.

## Reference host and corpus

- Apple M3 Max, arm64, 64 GiB RAM, macOS/Darwin 25.5.0.
- PostgreSQL 17 + pgvector 0.8.6 in the repository Compose service.
- Isolated database with one shared feed, one subscriber, 100,000 articles, and
  100,000 384-dimensional embeddings with the production HNSW index.
- Current debug server binary (a conservative latency measurement), Redis
  disabled for the HTTP run.

| Check | Result | Target |
|---|---:|---:|
| 100k article load | 7.197 s | evidence only |
| 100k embedding/HNSW load | 61.13 s | evidence only |
| Article-list SQL p50 / p95 / max (100 runs) | 30.684 / **32.478** / 37.960 ms | warm p95 <50 ms |
| Semantic SQL p50 / p95 / max (100 runs) | 92.104 / **102.343** / 136.928 ms | p95 <150 ms |
| Authenticated article JSON p50 / p95 / max (100 runs) | 58.914 / **63.807** / 174.415 ms | p95 <75 ms |
| Health JSON p50 / p95 / max (100 runs) | 0.760 / **0.906** / 0.948 ms | p95 <75 ms |
| Loaded server resident memory | **30,656 KiB** | <300 MiB |

The isolated benchmark and restore databases and temporary dump were removed
after the run. No user database was deleted or reset.

## Model and extraction

- `rustss-cli fetch-model` downloaded and initialized
  `BAAI/bge-small-en-v1.5` at 384 dimensions in a clean cache.
- Re-running with `HF_HUB_OFFLINE=1` used the same cache successfully.
- The real remote-CDP integration test rendered script-generated content while
  keeping publisher navigation behind `SafeFetchClient`.
- The unavailable-CDP test proves local article content remains available.

## Failure and recovery

- Redis-unavailable test: L1 + PostgreSQL behavior remains usable.
- Managed LLM test: a failed user provider falls through to an administrator
  provider and writes one shared summary.
- Chromium-unavailable test: local extraction remains readable.
- Worker interruption test: a missing heartbeat is reported stalled and an
  expired lock is reclaimed to pending.
- Job retry/dead-letter and lane-isolation tests remain green.

## Backup and restore

A PostgreSQL custom dump containing 100,000 articles, 100,000 embeddings, and
the then-current 33 migrations was restored into a fresh database. Restored
counts matched the source exactly; the compressed dump was 7.5 MiB.

On 2026-08-31 the restore drill was repeated with the current 41-migration
schema. A 319,544-byte custom dump restored into a newly created database,
`rustss-cli migrate` completed idempotently, and source/restored counts matched:
41 successful migrations, 178 users, 327 feeds, 123 articles, and 1,776
subscriptions. The production image reached readiness and served an
authenticated Library page using the restored database. The isolated restore
database, smoke container, and dump were then removed.

## Browser and security matrix

- Desktop 1440×900, tablet 834×1112, and mobile 390×844: Library, Feeds,
  Preferences, and AI Providers have no horizontal document overflow.
- The skip link receives keyboard focus and moves focus to the main region.
- Reduced-motion emulation reduces transitions to at most 1 ms.
- The authenticated shell carries a no-inline CSP, no inline event handlers or
  styles, `nosniff`, permissions policy, and no-referrer policy.
- The complete Playwright suite passed **18/18** flows with one deterministic
  worker, including catalog/OPML, Live View, consent-gated media/YouTube,
  sharing, drag/drop, side-panel scroll isolation, live priority summaries,
  admin queue refresh, rich typography, Arabic RTL persistence, recoverable
  personal CSS/Safe Mode, no-JavaScript fallback, and normal-user admin denial.

## Code and container gates

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --all-targets --all-features -- -D warnings`: clean.
- `cargo test --workspace --all-features`: green, followed by a serialized
  PostgreSQL/pgvector pass across every crate. That pass exposed two evolving
  form-compatibility defects; after correction, all affected web unit and
  integration binaries were rerun green, including the O(1) shared-content
  invariants.
- `docker build -f docker/Dockerfile`: green on arm64 using the Debian 13
  builder/runtime required by the bundled ONNX C++ ABI and WebAuthn's local
  OpenSSL verifier.
- The resulting image started as the non-root user and returned ready with
  PostgreSQL plus Redis. The current arm64 Docker save compressed with gzip
  level 1 is **94 MiB** (98,121,880 bytes); its uncompressed content is about
  259 MiB (271,759,715 bytes) and includes server, worker, and CLI binaries plus
  the declared WebAuthn/OpenSSL runtime.

## Release artifacts

`.github/workflows/release.yml` builds versioned Linux/macOS amd64/arm64 binary
archives, Linux amd64/arm64 Debian and RPM packages, SHA-256 manifests, SPDX
SBOMs, Sigstore bundles, and GitHub provenance. Its single multi-architecture
image is published to GHCR and, when configured, mirrored by exact digest to
Docker Hub and Quay; every registry digest is signed. CI inspects both package
formats, repeat-installs the Debian package, runs the packaged CLI, and starts
the published image against PostgreSQL until readiness succeeds. A semantic
version tag triggers publication; this repository task did not create or push a
release tag.
