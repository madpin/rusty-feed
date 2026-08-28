# RustSS — Findings, Smells & Risks (living document)

**Purpose:** a single running log of code smells, potential problems, bugs, and risks discovered while analyzing the reference `neureed` implementation and while building RustSS. Each entry has an ID, severity, status, and a proposed action.

**Process:**
- Add an entry the moment something questionable is found — don't rely on memory.
- Reference the entry ID in commits/PRs that address it (e.g., `fixes FND-003`).
- This document is **revisited and triaged at the end of Phase 1 (analysis/design)** and again at each milestone.
- Keep it honest: include risks in *our own* design, not just the reference's.

**Severity:** `critical` (data loss / security / corruption) · `high` (wrong behavior / perf cliff) · `medium` (smell / maintainability / minor bug) · `low` (nit / cosmetic).
**Status:** `open` · `planned` (has a design decision) · `in-progress` · `resolved` · `wontfix` · `monitor`.

---

## A. Reference implementation (`neureed`) — carried into design

These were found by reading the reference. RustSS's design already addresses most (cross-referenced to PRD §9 and the Architecture doc). Tracked here so nothing is silently forgotten during implementation.

| ID | Sev | Status | Finding | Action / where addressed |
|---|---|---|---|---|
| FND-001 | high | planned | **Conditional GET implemented but not wired.** `feed-parser` supports `If-None-Match`/`If-Modified-Since` + 304, but `feed-refresh-service` calls `parseFeedUrl(url)` with no options and never stores `etag`/`lastModified` (explicit TODO). Every refresh is a full GET. | Wire end-to-end in `FetchFeedAgent`; persist + reuse validators. PRD FR-REFRESH-3; ARCH §11/§8. |
| FND-002 | critical | planned | **Mixed-dimension embedding corruption.** Local (384-dim) vectors are zero-padded to 1536 and stored in the *same* `vector(1536)` column as OpenAI (1536) vectors. Mixing providers across articles silently corrupts cosine similarity. | **One embedding model per instance** + fixed-dim table + model guard (no pad/truncate). PRD FR-EMB-3; ARCH §13. |
| FND-003 | high | planned | **Cost tracking is in-memory only.** `embedding-cost-tracker` and `summarization-cost-tracker` use module-level arrays → lost on restart, not shared across instances, per-process. | Durable `cost_ledger` table. PRD FR-LLM-4; ARCH §14. |
| FND-004 | high | planned | **Two incompatible crypto schemes.** LLM keys use `lib/crypto.ts` (key = raw first 32 bytes of secret; format `iv:ct:tag`); feed cookies use `encryption-service.ts` (key = SHA-256(secret); format `iv:tag:ct`). Different key derivation AND field order. | One AEAD (XChaCha20-Poly1305 + HKDF); one-shot secret migration. PRD §9.4; ARCH §14/§21. |
| FND-005 | medium | planned | **`preserveStarred` is a no-op.** Cleanup accepts `preserveStarred: true` but there is no starred/bookmark model, so nothing is preserved. | Real `bookmarks` table honored by retention. PRD FR-JOB-3/4; ARCH §6/§11.6. |
| FND-006 | critical | resolved (M6) | **SSRF gaps beyond feed creation.** Only `createFeed` validated URLs; extraction/robots fetches only checked scheme — no private-IP blocking, no redirect re-validation (DNS-rebinding). | Done in `rustss-feeds::SafeFetchClient`: a custom reqwest DNS resolver rejects any host resolving to a non-public address (covers every redirect hop), plus IP-literal validation before connect. Extraction (M8) will reuse the same client. A `new_allow_private` variant exists for tests / explicitly-trusted intranet feeds. PRD FR-EXTRACT-5; ARCH §12. |
| FND-007 | medium | planned | **Job wiring is inconsistent.** Embedding-generation cron is defined but `startEmbeddingGenerationJob` is never called (dead). Pattern-decay is only API-triggered with no `JobLock`/`CronJobRun` tracking. Only feed-refresh + cleanup are actually scheduled. | All agents run through one framework w/ locks + run records. PRD FR-JOB-1/2; ARCH §8. |
| FND-008 | medium | planned | **`KEYS`-based cache invalidation** (`cacheDeletePattern` uses `KEYS pattern`) is O(N) and blocks Redis at scale. | Use `SCAN` + tag/prefix invalidation. PRD §9.8; ARCH §10. |
| FND-009 | medium | planned | **Cleanup ages by `createdAt`, not `publishedAt`.** Surprising for backfilled feeds (old articles just ingested get deleted immediately or kept wrongly). | Make age basis explicit/configurable (default `published_at`). PRD §9.9; ARCH §11.6. |
| FND-010 | high | planned | **System (scheduled) refresh passes no `userId`.** Result: no summaries generated on scheduled refresh, and embeddings always use the system/default (local) provider regardless of user config. Per-user enrichment only happens on manual user-scoped refresh. | In RustSS, enrichment is decoupled from the fetch and keyed by shared article (agents), not by user. Summaries are on-demand/optional-auto (admin-gated); embeddings use the single instance model. ARCH §7.3/§8. |
| FND-011 | medium | monitor | **URL dedup is global but GUID/hash dedup is feed-scoped.** An article URL present in two feeds collapses to one row owned by whichever feed inserted first; that article then "belongs" to a feed the other subscriber may not see as the source. | Preserve reference behavior (global URL uniqueness) but verify UX around "source feed" attribution. Consider an article↔feed association table if this proves confusing. Revisit. |
| FND-012 | low | monitor | **Near-duplicate detection defined but unused.** `calculateSimilarity`/`areNearDuplicates` (Jaccard, 0.8) exist but aren't called in the refresh path. | Decide whether to implement near-dup collapsing in RustSS or drop it. Low priority. |
| FND-013 | medium | planned | **Summary cost pricing table missing `gpt-4o-mini`** (the default model) → falls back to a `default` price → inflated cost estimates. | Include current model prices in the ledger pricing map; make it config-updatable. ARCH §14. |
| FND-014 | low | monitor | **Reference docs (CLAUDE.md) disagree with code** in several places (cost tracking "Redis" but it's in-memory; `feedMetadata` TTL "5 min" but code is 1h; summary TTL "1h" vs code 7d; search threshold 0.7 vs service 0.5). | RustSS docs use the **code-verified** values (ARCH §10/§11). Note for anyone cross-referencing the reference. |
| FND-015 | medium | planned | **Robots.txt path allow/deny is simplified** in the reference (`allowed: true` always; only crawl-delay is honored). | Decide RustSS policy: at minimum honor crawl-delay; optionally honor Disallow for extraction. Document the choice. |

---

## B. RustSS design/implementation — risks to watch

Risks introduced (or not yet closed) by our own choices. Revisit at each milestone.

| ID | Sev | Status | Finding | Action / mitigation |
|---|---|---|---|---|
| FND-101 | high | monitor | **Postgres-backed queue throughput ceiling.** `SKIP LOCKED`/apalis over Postgres is simple and durable but caps throughput vs. a dedicated broker. Fine for target scale (1–500 users), but verify against the ≥200 feeds/min + ≥1000 saved-search matches/min targets. | Benchmark early (ARCH §20). If needed, move hot queues to Redis (apalis-redis) while keeping the durable outbox. |
| FND-102 | medium | monitor | **HTMX vs. rich SPA interactions.** Resizable reading panel, drag-drop feed org, live theme switching, offline cache were SPA-native in the reference. Some need careful HTMX+Alpine design or graceful degradation. | Prototype the trickiest (reading panel resize, drag-drop categories) early; fall back to simpler UX if cost is high. PRD §2.2 non-goal (not pixel-identical). |
| FND-103 | high | monitor | **Model download on first run = network dependency + startup latency.** First run needs egress to fetch ONNX weights; air-gapped installs will fail without a pre-seeded `MODEL_CACHE_DIR`. | Document offline seeding; provide a `rustss-cli fetch-model` command; clear error if weights missing and network blocked. |
| FND-104 | medium | monitor | **Chromium resource use.** Headless Chromium is memory-hungry and can undermine the <300 MB loaded-RSS target if run in-process. | Run Chromium as a **separate** compose service (pooled, capped); app talks CDP over the network. Off by default is not an option (Full infra), but keep concurrency bounded. |
| FND-105 | high | planned | **GReader is a de-facto, unversioned spec.** Real clients rely on undocumented quirks; partial implementations cause subtle client breakage. | Scope to the Miniflux/FreshRSS-supported subset; add integration tests that replay real client request sequences; document supported endpoints. ARCH §9.1. |
| FND-106 | medium | monitor | **Embedding model change is disruptive** (full re-embed). If an admin swaps `EMBEDDING_MODEL`, search is degraded until backfill completes. | Make it an explicit, guarded admin flow with progress; keep old table until backfill done, then swap. ARCH §13.1. |
| FND-107 | medium | resolved | **`sqlx` compile-time query checks need a DB or offline cache** (`SQLX_OFFLINE` + `.sqlx/`). CI without a DB would fail unless offline metadata is committed. | **Resolved (M2):** use **runtime** `sqlx::query`/`query_as` (not the `query!` macros). Builds stay hermetic (no DB, no `.sqlx/`); SQL correctness is covered by integration tests against a real Postgres (CI runs a pgvector service). |
| FND-111 | low | planned | **Preferences stored as a JSONB blob** (`user_preferences.preferences`) rather than the 30+ typed columns the Architecture describes. | Deliberate: a typed Rust struct (serde) will define the shape in M5 and (de)serialize to JSONB, avoiding migration churn. Revisit if we need SQL-level querying of individual preferences. |
| FND-112 | medium | monitor | **`article_embeddings.embedding` dimension is hardcoded to `vector(384)`** in the initial migration (the default local BGE-small model). An instance configured for OpenAI (1536) needs a different dimension. | Aligns with the "one model per instance; model change = explicit re-embed" policy (ARCH §13.1). M9 adds a guarded re-dimension/backfill flow; document the OpenAI setup path in `docs/MIGRATION.md`. |
| FND-108 | medium | monitor | **Content sanitization vs. fidelity.** Aggressive `ammonia` allow-lists can strip legitimate embeds (iframes for video, etc.); too permissive risks XSS. | Curate an allow-list tuned for feed content (safe iframes for known video hosts behind the media proxy); test with real feeds. |
| FND-109 | low | open | **UUIDv7 vs. reference cuid/nanoid IDs.** Migration must map or preserve old string IDs; mixing formats needs care in FKs. | Decide in `docs/MIGRATION.md`: keep old IDs as `text` for migrated rows, UUIDv7 for new; or remap via translation tables. |
| FND-110 | medium | monitor | **Two crypto/secret classes at rest** (API keys, feed cookies) all hinge on one `ENCRYPTION_SECRET`. Losing/rotating it invalidates all secrets. | Document rotation procedure; support re-encrypt-on-read with a versioned key id (AEAD associated data includes a key version). |
| FND-114 | low | planned | **No cross-instance L1 cache invalidation (M3).** `delete`/`delete_prefix` clear the local moka L1 + Redis L2, but a *different* app instance's L1 stays warm until its per-entry TTL. Single-instance (default self-hosted) is exact; multi-instance L1 can be briefly stale. | Add Redis pub/sub: on invalidation, publish the key/prefix; each instance subscribes and drops matching L1 entries. Defer until multi-instance is actually used. TTLs bound staleness meanwhile. |
| FND-118 | low | monitor | **Curated hand-written CSS instead of a Tailwind build pipeline (M5).** The architecture/README name Tailwind; to avoid a Node/standalone-binary build dependency (offline self-hosting) we ship a small design-system stylesheet (CSS variables, components, light/dark/system) embedded via rust-embed, plus a tiny dependency-free `app.js` for progressive enhancement (in-place actions, inline reading). | Revisit if the design outgrows hand-written CSS; a Tailwind (standalone CLI) build can be added at that point. Update README stack wording when decided. |
| FND-117 | low | planned | **OAuth implemented directly over `reqwest` instead of the `oauth2` crate (M4b).** Authorization-code flow with PKCE (S256) + `state` CSRF, provider profile fetched from userinfo; supports Google, GitHub, and a generic OIDC provider. | Deliberate: TLS control (rustls, no OpenSSL) and hermetic testability (wiremock). Security-sensitive — covered by integration tests (happy path + state-mismatch rejection). Reconsider the crate if we need broader OIDC features (discovery, id_token validation). Docs updated (ARCH §15). |
| FND-116 | low | planned | **Custom session scheme instead of `tower-sessions` (M4a).** The Architecture named `tower-sessions` + a sqlx store; we implemented a small DB-backed scheme (opaque token in an HTTP-only cookie; SHA-256 hash stored in `sessions`) to avoid session-library version churn and keep full control. | Deliberate. Satisfies FR-AUTH-3 (server-side sessions in Postgres). Revisit if we need session features (rolling stores, layered middleware) that the library provides. Docs updated (ARCH §15). |
| FND-115 | medium | resolved | **Compose commands did not load the root `.env`.** Because the Compose file is under `docker/`, commands without an explicit env file silently ignored root-level host-port overrides and secrets. | Standardized all repository commands on `docker compose --env-file .env -f docker/compose.yaml …`; documented host vs. container ports and added Compose port variables to `.env.example`. |

---

## C. Triage log

- **2026-08-28** — Initial population from Phase 1 analysis (FND-001…015) and initial design-risk pass (FND-101…110). To be triaged at end of Phase 1.

---

*Cross-references: PRD `docs/PRD.md` §9 (deficiencies), Architecture `docs/ARCHITECTURE.md` §7/§8/§10/§11/§12/§13/§14/§21.*
