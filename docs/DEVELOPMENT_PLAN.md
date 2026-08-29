# RustSS — Development Plan (for a code agent)

**Audience:** an autonomous (or human) engineer implementing RustSS from the PRD + Architecture.
**Companion docs:** `docs/PRD.md`, `docs/ARCHITECTURE.md`, `docs/FINDINGS.md`.
**Golden rule to never violate:** *content is shared, state is personal* (PRD §6, ARCH §7). Every schema/agent/query decision must preserve O(1) shared work for N subscribers.

---

## How to use this plan

- Work **milestone by milestone** (M0 → M13). Each milestone is independently shippable/reviewable and ends with a **Conventional Commit**.
- Before coding a milestone: re-read the referenced ARCH/PRD sections and the relevant `FND-*` entries.
- After coding: run the milestone's **Verification** and check the **Definition of Done (DoD)**.
- Keep `docs/FINDINGS.md` updated whenever you hit a smell/bug/risk (reference the `FND-*` id in the commit).
- Prefer many small commits within a milestone over one giant commit.

### Global Definition of Done (applies to every milestone)
- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean
- [ ] `cargo test` (workspace) green
- [ ] Public items documented (`///`); no `unwrap()`/`expect()` on fallible paths outside tests/bins startup
- [ ] New env/config documented in `.env.example` + ARCH §16
- [ ] New findings/risks recorded in `docs/FINDINGS.md`
- [ ] Conventional Commit with a body explaining **why**

### Conventions
- Errors: `thiserror` in libs, `anyhow` in bins/agents boundaries (ARCH §19).
- SQL: `sqlx` **runtime** queries (`query`/`query_as` + `FromRow`), not the `query!` macros — hermetic builds; SQL validated by integration tests against real Postgres (FND-107).
- IDs: UUIDv7. Time: `time` + `timestamptz`. Money/cost: store integer micro-USD or `numeric`.
- All outbound HTTP goes through `SafeFetchClient` (ARCH §12). No exceptions.
- All per-user queries are scoped by `user_id`. Shared writes are idempotent (`ON CONFLICT`).

---

## Milestone map

| M | Title | Outcome | Key crates |
|---|---|---|---|
| M0 | Workspace & CI scaffold | Builds, lints, tests, CI green | all (skeletons) |
| M1 | Config, errors, telemetry | Typed config, error taxonomy, tracing/metrics | `core` |
| M2 | Database & migrations | Full schema, repos, pgvector, testcontainers | `db` |
| M3 | Cache layer | moka L1 + Redis L2, key/TTL/invalidation | `cache` |
| M4 | Auth & sessions | OAuth + local, sessions, RBAC, CSRF | `web` |
| M5 | Web shell & reading UI | Askama+HTMX layout, article list/read, prefs | `web` |
| M6 | Feeds: fetch/parse/dedup + SafeFetch | Add feed, ingest, conditional GET, SSRF | `feeds` |
| M7 | AGENTS framework | Scheduler, queue, locks, run records, outbox | `agents` |
| M8 | Content extraction | Readability + Chromium, rate limit, robots, sanitize | `extract` |
| M9 | Embeddings & semantic search | Local ONNX (download-on-first-run), pgvector search | `embed`, `search` |
| M10 | Personalization | Patterns, feedback, scoring, decay | `search` |
| M11 | Saved searches | DSL/AST, matcher agent, notifications | `search`, `agents` |
| M12 | LLM summarization + cost ledger | Summaries, provider cascade, durable costs | `llm` |
| M13 | Admin, OPML, feed health, bulk ops, GReader API | Ops surface + client interop | `web`, `agents` |

Post-v1: v1.1 (bookmarks/seen/hybrid/OPML fixes), v2 (clustering+digests, shortcuts, i18n, ADHD mode).

---

## M0 — Workspace & CI scaffold
**Goal:** a compiling multi-crate workspace with tooling, no business logic.
**Refs:** ARCH §5, §18.

Tasks:
- [ ] Root `Cargo.toml` workspace; `rust-toolchain.toml` (pin stable); `.rustfmt.toml`, `clippy` config.
- [ ] Create crates (lib skeletons): `core`, `db`, `cache`, `feeds`, `extract`, `embed`, `llm`, `search`, `agents`, `web`; bins: `server`, `worker`, `cli`.
- [ ] `.env.example`, `docker/Dockerfile`, `docker/compose.yaml` (postgres+pgvector, redis, chromium, app), `docker/entrypoint.sh`.
- [ ] GitHub Actions CI: fmt + clippy(-D warnings) + test + build; cache cargo; Postgres+pgvector service for DB integration tests.
- [ ] `AGENTS.md`, `CLAUDE.md` present (done in docs phase).

Verification: `cargo build` + `cargo clippy` + `cargo test` green; `docker build` succeeds.
DoD commit: `chore: scaffold cargo workspace, tooling, and CI`.

---

## M1 — Config, errors, telemetry
**Goal:** foundation every crate depends on.
**Refs:** ARCH §16, §17, §19; FND-107.

Tasks:
- [ ] `core::config::AppConfig` via `figment` (env + optional file), typed + validated at startup; fail-fast on missing required. Cover all env groups in ARCH §16 (DB, Redis, auth, embeddings incl. `EMBEDDING_MODEL`/`MODEL_CACHE_DIR`, LLM, extraction/Chromium, jobs, crypto, server).
- [ ] `core::error` — domain error enums (`thiserror`); `web::error::AppError: IntoResponse` mapping (400/401/403/404/409/429/500) with safe messages.
- [ ] `core::telemetry` — `tracing-subscriber` init (JSON/pretty), `tower-http::TraceLayer` wiring helper, request-id, `metrics` + Prometheus `/metrics` exporter.
- [ ] `core::ports` — define the trait "ports" (Repos, `Cache`, `VectorStore`, `Embedder`, `LlmClient`, `FetchClient`, `Queue`, `Clock`) as stubs for later crates to implement.

Verification: unit tests for config parse (valid/invalid); a tiny bin prints resolved config with secrets masked.
DoD commit: `feat(core): typed config, error taxonomy, tracing + metrics`.

---

## M2 — Database & migrations
**Goal:** the complete schema + typed repositories; the heart of the dedup model.
**Refs:** ARCH §6, §7; PRD §6; FND-002/005/009/011.

Tasks:
- [ ] SQL migrations (`crates/db/migrations`) implementing ARCH §6 groups: users/auth (incl. `api_tokens`), feeds (shared) + `feed_error_log` + `domain_extraction_state`, articles (shared) + `article_embeddings` (fixed dim from `EMBEDDING_MODEL`; HNSW `vector_cosine_ops`), subscriptions/categories, per-user state (`read_articles`, `seen_articles`, `bookmarks`, `article_feedback`, `user_patterns`), saved searches (+matches), preferences, notifications, ops (`admin_settings`, `cost_ledger`, `agent_runs`, `agent_locks`, `job_queue`, `outbox_events`).
- [ ] Enable extensions: `CREATE EXTENSION vector`. Generated `tsvector` column + GIN on articles.
- [ ] Indexes per ARCH §6.2 (unique URLs, `(feed_id, published_at DESC)`, HNSW, GIN tags, job_queue partial index).
- [ ] Repositories (`crates/db`, one module per aggregate) with runtime `query_as` + `FromRow`; **idempotent upserts** for feeds/articles (ON CONFLICT) proving FND dedup.
- [ ] `rustss-cli migrate` (apply) + entrypoint hook.
- [ ] Test infra: `sqlx::test` + testcontainers Postgres/pgvector helper.

Verification (critical):
- [ ] Integration test: creating the same feed URL twice yields **one** `feeds` row + two `user_feeds` rows.
- [ ] Integration test: ingesting the same article (GUID→URL→hash order) is idempotent; `should_update` logic matches ARCH §11.1.
- [ ] Unsubscribe removes only per-user rows; shared content remains.
DoD commit: `feat(db): schema + repositories with shared/per-user dedup model`.

---

## M3 — Cache layer
**Goal:** two-tier cache with correct key separation.
**Refs:** ARCH §10; FND-008.

Tasks:
- [ ] `cache::Cache` impl: moka L1 + `fred` Redis L2; `cache_get_or_set`, typed (serde) values, per-namespace TTLs from ARCH §10.
- [ ] `SCAN`-based prefix/tag invalidation (never `KEYS`); Redis pub/sub to drop L1 across instances.
- [ ] Graceful degradation: Redis down ⇒ L1+source still work (feature-tested).
- [ ] Key builders with **shared vs per-user** separation (compile-time helpers).

Verification: unit tests for get/set/ttl/invalidation; a test simulating Redis outage.
DoD commit: `feat(cache): moka L1 + redis L2 with SCAN invalidation`.

---

## M4 — Auth & sessions
**Goal:** users can sign in (OAuth + local), sessions + RBAC + CSRF.
**Refs:** ARCH §15, §9; PRD §5.1; FND-110.

Tasks:
- [ ] `tower-sessions` + Postgres store; secure cookie config.
- [ ] Local accounts: register/login, Argon2id; `REGISTRATION_MODE` (open/invite/closed); first-user-becomes-admin bootstrap.
- [ ] OAuth (`oauth2`): Google, GitHub, Generic OIDC; account linking (`oauth_accounts`).
- [ ] RBAC extractor (`ADMIN|USER|GUEST`); admin-route guard.
- [ ] CSRF middleware for cookie-auth mutations; `tower_governor` rate limit on auth endpoints.
- [ ] `api_tokens`: create/revoke app tokens (foundation for GReader in M13).
- [ ] On user create: subscribe to default feeds (idempotent) — depends on M6 feed create; stub then wire.

Verification: handler tests for register/login/logout, role gating, CSRF rejection, rate limiting.
DoD commit: `feat(web): oauth + local auth, sessions, rbac, csrf`.

---

## M5 — Web shell & reading UI
**Goal:** the server-rendered HTMX experience for reading.
**Refs:** ARCH §9; PRD §5.10, §4.3.

Tasks:
- [ ] Askama base layout; Tailwind build pipeline (standalone CLI at build); `rust-embed` assets; HTMX + Alpine included.
- [ ] Theme + typography applied **server-side** from `user_preferences` (14 themes → CSS variables).
- [ ] Article list (filter by feed/category/unread/search; sort by date/relevance; pagination + infinite scroll partial).
- [ ] Reading modes: side panel (resizable via Alpine), inline (accordion), standalone page.
- [ ] Mark-as-read (auto + manual), read/unread toggle; feedback buttons (👍/👎) posting to per-user tables.
- [ ] Preferences page (all 30+ fields) with progressive-enhanced forms.
- [ ] Notification bell + list (polling partial).

Verification: handler/template tests; snapshot a couple of rendered partials; no-JS smoke (form posts work).
DoD commit: `feat(web): htmx reading UI, reading modes, preferences`.

---

## M6 — Feeds: fetch/parse/dedup + SafeFetch
**Goal:** add feeds and ingest articles correctly and safely (no agents yet — call synchronously or via a manual trigger).
**Refs:** ARCH §11.1, §12; PRD §5.2/§5.3; FND-001/006/011.

Tasks:
- [ ] `feeds::SafeFetchClient` (reqwest + rustls): scheme allow-list, DNS + private/loopback/link-local block (v4/v6), redirect re-validation, size cap, timeouts. **Unit-test the SSRF matrix.**
- [ ] Feed URL normalization + `isSafeFeedUrl` equivalent; create-feed dedup by normalized URL (shared) + `user_feeds` subscribe.
- [ ] `feed-rs` parsing (RSS/Atom/JSON Feed); map to `ParsedArticle`.
- [ ] **Conditional GET**: send/store/reuse `ETag`/`Last-Modified`; handle 304 (FND-001).
- [ ] Dedup + upsert (`article-deduplication` + `article-service` equivalents) per ARCH §11.1; `content_hash = sha256(trim(content))`.
- [ ] Feed add/remove/list UI + native API endpoints.

Verification: `wiremock` feed server tests (200 with items, 304, redirects, malformed); SSRF unit tests; dedup integration test (N users ⇒ 1 fetch path).
DoD commit: `feat(feeds): safe fetch, parse, conditional GET, dedup ingest`.

---

## M7 — AGENTS framework
**Goal:** the background runtime that guarantees O(1) shared work.
**Refs:** ARCH §8, §7.3; PRD §5.12; FND-007/101.

Tasks:
- [ ] `agents::Agent` trait + `AgentContext` (injected ports) + typed `Job` + `AgentOutcome`.
- [ ] Runtime: cron scheduler (apalis/tokio-cron), Postgres queue consumer (`SKIP LOCKED`/apalis), bounded concurrency.
- [ ] Singleton locks via `agent_locks` lease + advisory locks (`JOB_LOCK_TIMEOUT` default 10m).
- [ ] Run tracking → `agent_runs` (status/duration/stats/logs/trigger); `JobLogger` ring buffer.
- [ ] Outbox dispatcher: `outbox_events` → queue jobs (+ optional Redis nudge).
- [ ] Wire **FeedRefreshAgent** (compute due feeds, dedup, enqueue) + **FetchFeedAgent** (uses M6) into the runtime; emit `ArticlesIngested`.
- [ ] `rustss-worker` bin; `RUN_EMBEDDED_WORKER` in `rustss-server`.

Verification: test that a feed subscribed by N users enqueues **one** fetch; lock prevents double-run; a failing job is retried/backed-off and recorded.
DoD commit: `feat(agents): scheduler, durable queue, locks, run records, outbox`.

---

## M8 — Content extraction
**Goal:** enrich new articles with full text, safely and politely.
**Refs:** ARCH §12; PRD §5.4; FND-006/104/108/015.

Tasks:
- [ ] `extract::Extractor` trait + registry (priority + fallback).
- [ ] Readability via `article_scraper`; Chromium via `chromiumoxide` (separate compose service, pooled, `CHROMIUM_ENABLED`).
- [ ] Per-domain rate limiter (`domain_extraction_state`): base 3s, robots.txt crawl-delay (cache 24h/1h-err), adaptive 429 backoff 10/30/60/120 honoring `Retry-After`.
- [ ] Per-feed extraction settings (method/headers/**encrypted cookies**/selector/timeout/merge strategy).
- [ ] `ammonia` sanitization before store + at render; media handling.
- [ ] **ExtractContentAgent** (event-driven on `ArticlesIngested`, new articles only, once per shared article).

Verification: `wiremock` extraction tests (html, 429+Retry-After, robots crawl-delay); sanitizer XSS tests; SSRF applies to extraction fetches.
DoD commit: `feat(extract): readability + chromium, rate limiting, sanitize`.

---

## M9 — Embeddings & semantic search
**Goal:** one embedding space per instance + fast vector search.
**Refs:** ARCH §13, §11.3; PRD §5.5; FND-002/103/106.

Tasks:
- [ ] `embed::Embedder`: local `fastembed` (BGE-small-384) with **download-on-first-run** into `MODEL_CACHE_DIR`; OpenAI-compatible via `async-openai`. `rustss-cli fetch-model`.
- [ ] `VectorStore` (pg): insert into `article_embeddings` (fixed dim, model guard, `ON CONFLICT DO NOTHING`); KNN query with HNSW.
- [ ] Text prep `title\n\nexcerpt\n\ncontent[..2000]`; batch (default 10).
- [ ] `search::semantic`: similarity `1 - dist/2`; optional recency blend `(1-w)*sim + w*exp(-age/decay)`; filters; related-articles.
- [ ] Hybrid keyword via `tsvector` (P1).
- [ ] **EmbedArticleAgent** (event-driven, once per article) + backfill mode.
- [ ] Search UI + native API.

Verification: embedding determinism test (local); model-guard rejects wrong dim (FND-002); search returns expected ordering on a seeded corpus; first-run download smoke (network) + clear error when weights missing offline (FND-103).
DoD commit: `feat(embed,search): single-space embeddings + semantic search`.

---

## M10 — Personalization
**Goal:** per-user learning + relevance ranking.
**Refs:** ARCH §11.2; PRD §5.6.

Tasks:
- [ ] `search::patterns`: TF keyword extraction (stop-words, len>2, heuristic weights) exactly per ARCH §11.2.
- [ ] Feedback service: explicit ±1.0, implicit completion +0.5 / bounce −0.5 (user `bounce_threshold`); explicit never overwritten.
- [ ] Pattern update `Δw = fb·rel·0.1`; decay `0.9^floor(days/30)`; prune `|w|<0.1`; top 100.
- [ ] Scoring: sigmoid `1/(1+e^(-5·Σ))`, neutral 0.5; cache `score:{user}:{article}` 1h + explanations; batch scoring.
- [ ] **PatternDecayAgent** (scheduled, tracked+locked — fixes FND-007).
- [ ] Wire ranking/filter into the article list (M5); "Reset Learning".

Verification: unit tests with exact expected values for extraction/update/decay/prune/sigmoid (regression vs. reference constants).
DoD commit: `feat(search): personalization patterns, feedback, scoring`.

---

## M11 — Saved searches
**Goal:** persistent boolean+semantic queries auto-matching new articles.
**Refs:** ARCH §11.4, §8; PRD §5.7; FND-105 (adjacent).

Tasks:
- [ ] `search::dsl`: parser → AST (OR `,`, AND `+`, NOT `-`, phrase `"..."`, group `()`); `proptest` coverage.
- [ ] Executor: boolean filter + keyword score (`1+ln(occ)`, ×2 title) + semantic; combined `0.6·sem+0.4·kw`; threshold 0.6.
- [ ] **SavedSearchMatchAgent** (event-driven on new/embedded articles): pre-embed query per batch (100 articles × concurrency 5); upsert matches; notify ≥ 0.85; rematch (90d); cleanup (90d); templates.
- [ ] CRUD UI + native API; results with pagination.

Verification: parser property tests; matcher integration test (throughput sanity ≥ target); notification on high-relevance.
DoD commit: `feat(search,agents): saved searches DSL + matcher`.

---

## M12 — LLM summarization + cost ledger
**Goal:** optional AI summaries with durable costs.
**Refs:** ARCH §14, §11; PRD §5.8; FND-003/004/013.

Tasks:
- [ ] `llm::LlmClient`: OpenAI-compatible (`async-openai`) + Ollama; provider cascade user→admin→env.
- [ ] Secrets: XChaCha20-Poly1305 + HKDF-SHA256 unified AEAD (fixes FND-004); encrypt user/system API keys + feed cookies; masking; key-version in AAD (FND-110).
- [ ] Summarize → JSON {summary,key_points,topics}; robust parse + fallback; store once on shared article (first-writer-wins) with short lock.
- [ ] `cost_ledger` durable accounting (per user/provider/model/day); pricing map incl. `gpt-4o-mini` (FND-013).
- [ ] **SummarizeArticleAgent** (on-demand + optional admin-gated auto on refresh, min content length).
- [ ] Summary UI (on article) + admin cost views.

Verification: `wiremock` LLM tests; AEAD round-trip + tamper-detection tests; first-writer-wins concurrency test; cost math test.
DoD commit: `feat(llm): summarization + durable cost ledger + unified secrets`.

---

## M13 — Admin, OPML, feed health, bulk ops, GReader API
**Goal:** operability + open-standard client interop.
**Refs:** ARCH §9.1, §17; PRD §5.2/§5.9/§5.11/§5.13; FND-105.

Tasks:
- [x] Curated feed magazine: starter sources, country/category/language facets, derived feed stats, multi-select subscription, admin publishing/reordering, and unrestricted custom feed URLs.
- [x] Configurable keyboard shortcuts: capture/reset controls, server-side validation, and safe Library article/feed actions.
- [x] Personal feed organization: nested folders, multi-folder assignment, normalized tags, scoped Library/mark-all-read filters, cached source icons, and a persisted reduced sidebar.
- [ ] Feed health tracking (states, consecutive failures, auto-disable, error log, notifications) integrated with FetchFeedAgent.
- [ ] Bulk feed operations (category/tags/settings/enable-disable/delete/refresh) with partial-failure + authz.
- [ ] OPML import/export (categories preserved; category-selection import — fixes reference bug).
- [ ] CleanupAgent (retention, **preserve bookmarks**, configurable age basis; VACUUM/ANALYZE) — FND-005/009.
- [~] Admin dashboard: overview counts, user/admin-role management, shared-feed inventory, curated feed magazine editing, and cost visibility are implemented. Storage/memory, job history/manual triggers, provider config, constraints, and maintenance actions remain.
- [ ] **GReader API adapter** under `/api/greader/` (ARCH §9.1): ClientLogin + T token (via `api_tokens`), subscription/list+edit, tag/list, stream/contents + items/ids + items/contents, edit-tag, mark-all-as-read, user-info. Map read→`read_articles`, star→`bookmarks`, labels→categories.
- [ ] GReader integration tests replaying real client request sequences (FND-105).

Verification: admin handler tests; OPML round-trip; health-state transitions; GReader conformance tests (subset).
DoD commit(s): `feat(web): admin, opml, feed health, bulk ops` + `feat(api): greader-compatible client API`.

---

## Post-v1

- **v1.1:** bookmarks/favorites UI polish, "seen" state, hybrid search UX, OPML category-selection, batched health notifications, media proxy, registration invites.
- **v2:** ClusterAgent (group related articles) + DigestAgent (LLM event/daily digests, optional email), i18n/message catalogs, ADHD reading formatting, touch gestures, date-format prefs, auto-link bare URLs, YouTube embeds, storage/log admin tooling.

---

## Sequencing & parallelization notes
- **Critical path:** M0→M1→M2 then M6 (feeds) and M7 (agents) unlock the enrichment agents (M8/M9/M11/M12).
- **Parallelizable after M2:** M3 (cache), M4 (auth), M5 (web shell) can proceed alongside M6.
- **M9 before M11** (saved-search matcher uses embeddings). **M6+M7 before M8** (extraction is an agent on ingested articles).
- Keep a running **benchmark harness** from M6 onward to guard PRD §7 targets (FND-101).
- Each milestone must preserve the dedup invariants — add/keep the M2 dedup tests green throughout.
