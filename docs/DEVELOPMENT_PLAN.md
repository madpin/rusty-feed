# Rusty Feed — Development Plan (for a code agent)

**Audience:** an autonomous (or human) engineer implementing Rusty Feed from the PRD + Architecture.
**Companion docs:** `docs/PRD.md`, `docs/ARCHITECTURE.md`, `docs/FINDINGS.md`.
**Golden rule to never violate:** *content is shared, state is personal* (PRD §6, ARCH §7). Every schema/agent/query decision must preserve O(1) shared work for N subscribers.
**Status:** all v1 implementation milestones complete; release-candidate audit performed on **2026-08-31**. Public release publication still requires a maintainer-created version tag.

---

## How to use this plan

- Work **milestone by milestone** (M0 → M13). Each milestone is independently shippable/reviewable and ends with a **Conventional Commit**.
- Before coding a milestone: re-read the referenced ARCH/PRD sections and the relevant `FND-*` entries.
- After coding: run the milestone's **Verification** and check the **Definition of Done (DoD)**.
- Keep `docs/FINDINGS.md` updated whenever you hit a smell/bug/risk (reference the `FND-*` id in the commit).
- Prefer many small commits within a milestone over one giant commit.

### Global Definition of Done (applies to every milestone)
- [x] `cargo fmt --all --check` clean
- [x] `cargo clippy --all-targets --all-features -- -D warnings` clean
- [x] `cargo test` (workspace) green
- [x] Public items documented (`///`); no `unwrap()`/`expect()` on fallible paths outside tests/bins startup
- [x] New env/config documented in `.env.example` + ARCH §16
- [x] New findings/risks recorded in `docs/FINDINGS.md`
- [ ] Conventional Commit with a body explaining **why**

### Conventions
- Errors: `thiserror` in libs, `anyhow` in bins/agents boundaries (ARCH §19).
- SQL: `sqlx` **runtime** queries (`query`/`query_as` + `FromRow`), not the `query!` macros — hermetic builds; SQL validated by integration tests against real Postgres (FND-107).
- IDs: UUIDv7. Time: `time` + `timestamptz`. Money/cost: store integer micro-USD or `numeric`.
- All outbound HTTP goes through `SafeFetchClient` (ARCH §12). No exceptions.
- All per-user queries are scoped by `user_id`. Shared writes are idempotent (`ON CONFLICT`).

---

## Milestone map

| M | Status | Title | Evidence and remaining gate |
|---|---|---|---|
| M0 | ✅ | Workspace & CI scaffold | Workspace, containers, CI, Rust and browser suites are operational. |
| M1 | ✅ | Config, errors, telemetry | Typed environment/TOML configuration, secret-safe diagnostics, safe web errors, tracing, bounded HTTP metrics, readiness, and narrow feature-owned interfaces are implemented and tested. |
| M2 | ✅ | Database & migrations | 51 migrations, runtime SQLx repositories, pgvector, protocol aliases, managed AI/integration/acquisition profiles, passkeys, rewrite history, the persisted automatic-summary policy, and database-backed dedup/tenant tests are present. Integration tests use an explicit test database instead of hidden build-time SQL. |
| M3 | ✅ | Cache layer | moka L1, namespace-isolated optional Redis L2, bounded TTLs, typed keys, `SCAN` invalidation, versioned cross-instance Pub/Sub coherence, and graceful degradation are implemented and tested. |
| M4 | ✅ | Auth & sessions | Local/OAuth/passkey accounts, invitation lifecycle, trusted-proxy boundary, first-admin/bootstrap CLI, default subscriptions, opaque session listing/revocation, RBAC, CSRF, rate limiting, and API tokens are implemented and tested. |
| M5 | ✅ | Web shell & reading UI | The strict-CSP responsive reader, three modes, preferences, versioned CSS/managed opt-in JavaScript with Safe Mode, 20 locale/format profiles, live/batched notifications, PWA shell, no-JS fallbacks, and desktop/tablet/mobile accessibility matrix are implemented and tested. Complete translation coverage remains a separate parity row. |
| M6 | ✅ | Feeds: fetch/parse/dedup + SafeFetch | Safe redirects/DNS, RSS/Atom/JSON Feed, conditional GET, shared dedup, feed UI, and administrator-owned host-pinned cookie/proxy/certificate acquisition profiles are covered by unit and PostgreSQL tests. |
| M7 | ✅ | AGENTS framework | Durable lane-isolated workers, shared agents, leases, retries, heartbeats, sanitized run records, UTC cron schedules, and transactional outbox dispatch are implemented and tested. |
| M8 | ✅ | Content extraction | Shared Readability, selectors, ordered/versioned rewrite rules, personal bounded entry filters, robots/rate limits, sanitization, access/rule refresh, content lifetimes, cleanup, and a bounded remote-CDP fallback are wired. Chromium receives only HTML already obtained through `SafeFetchClient`; integration tests prove inline rendering and graceful isolation. |
| M9 | ✅ | Embeddings & semantic search | Fixed-space pgvector, production local BGE/fastembed with offline cache seeding, OpenAI-compatible embeddings, explicit hash fallback, guarded reindex/model migration, semantic/related/hybrid search, agents, and native API are implemented and tested. |
| M10 | ✅ | Personalization | Explicit/implicit feedback, exact scoring/decay, caching, agent scheduling, ranking, explanations, and reset are implemented and tested. |
| M11 | ✅ | Saved searches | Property-tested DSL, hybrid matcher, durable matches/notifications, agents, full CRUD, pagination, retention/rematch controls, and native resources are implemented and tested. |
| M12 | ✅ | LLM summarization + cost ledger | OpenAI/Ollama-compatible summaries, user → admin → environment provider cascade, encrypted managed keys, shared first-writer storage, reserved interactive lane, live UI, automatic policy, sanitized errors, and durable costs are implemented and tested. |
| M13 | ✅ | Admin, OPML, feed health, bulk ops, client APIs | Discovery, OPML, organization, public shares, health/pause/bulk controls, retention, storage/queue maintenance, YouTube duration metadata, webhook/destination delivery, native resources, and GReader/Fever subsets with replay tests are implemented. |

All fourteen milestones meet their v1 product outcome and verification bar.
The repository is a release candidate; pushing the intended `vX.Y.Z` tag is a
separate maintainer-controlled publication action.

Seen state, shared story clustering, personal briefings, ADHD reading format, bare-URL linking, password reset, and library infinite scroll are implemented. The expanded Miniflux audit is 65/67 complete, with full translation coverage and exact turnkey adapters for all advertised providers still partial. Offline article storage and the separate Rusty Signal service remain out of this repository.

Miniflux ecosystem parity is tracked across milestones in
[`docs/MINIFLUX_PARITY.md`](MINIFLUX_PARITY.md). Any task that implements or
changes a parity capability must update that row's status and evidence in the
same change; the tracker's recommended delivery order guides new milestone
planning after M13.

---

## M0 — Workspace & CI scaffold
**Goal:** a compiling multi-crate workspace with tooling, no business logic.
**Refs:** ARCH §5, §18.

Tasks:
- [x] Root `Cargo.toml` workspace; `rust-toolchain.toml` (pin stable); `rustfmt.toml`; CI-enforced clippy configuration.
- [x] Create crates: `core`, `db`, `cache`, `feeds`, `extract`, `embed`, `llm`, `search`, `agents`, `web`; bins: `server`, `worker`, `cli`.
- [x] `.env.example`, `docker/Dockerfile`, `docker/compose.yaml` (postgres+pgvector, redis, chromium, app), `docker/entrypoint.sh`.
- [x] GitHub Actions CI: fmt + clippy (`-D warnings`) + database-backed tests + release build + Playwright; cached Cargo and PostgreSQL/pgvector/Redis services. Playwright uses a dedicated database and deterministic test embedding space so integration-test vectors cannot contaminate server startup (FND-200).
- [x] `AGENTS.md` and compatibility pointers present.

Verification: `cargo build` + `cargo clippy` + `cargo test` green; `docker build` succeeds.
DoD commit: `chore: scaffold cargo workspace, tooling, and CI`.

---

## M1 — Config, errors, telemetry
**Goal:** foundation every crate depends on.
**Refs:** ARCH §16, §17, §19; FND-107.

Tasks:
- [x] `core::config::AppConfig` via `figment`, typed and validated at startup. Environment values override an optional TOML file, every group is documented in `.env.example` and `docs/CONFIGURATION.md`, and an explicitly missing file fails clearly.
- [x] `core::error` domain errors plus the single safe `web::error::AppError: IntoResponse` mapping.
- [x] `core::telemetry` tracing, request tracing, readiness/health, metrics, and Prometheus `/metrics`.
- [x] Narrow substitution interfaces live with their domain types (`Embedder`, `LlmClient`, `Extractor`, and `Agent`); concrete repositories/cache/queue remain direct adapters until a real second implementation or test seam exists. This deliberately replaces the obsolete catch-all `core::ports` stub proposal (FND-160).

Verification: unit tests cover valid/invalid configuration, missing optional files, and diagnostic redaction; `rusty-feed-cli config-check` prints only a secret-safe resolved summary. HTTP request metrics use route templates rather than raw paths.
DoD commit: `feat(core): typed config, error taxonomy, tracing + metrics`.

---

## M2 — Database & migrations
**Goal:** the complete schema + typed repositories; the heart of the dedup model.
**Refs:** ARCH §6, §7; PRD §6; FND-002/005/009/011.

Tasks:
- [x] SQL migrations implement the shared, personal, auth, search, notification, operations, media, catalog, and public-share schema groups.
- [x] Enable pgvector and generated weighted `tsvector` columns with GIN indexes.
- [x] Required uniqueness, feed/article, vector, tag, and queue indexes.
- [x] Runtime SQLx repositories with typed rows and idempotent shared upserts.
- [x] `rusty-feed-cli migrate` plus entrypoint migration hook.
- [x] Explicit `TEST_DATABASE_URL` PostgreSQL/pgvector integration-test infrastructure; CI supplies the real service. This intentionally replaces a hidden testcontainers dependency.

Verification (critical):
- [x] Integration test: creating the same feed URL twice yields **one** `feeds` row + two `user_feeds` rows.
- [x] Integration test: ingesting the same article (GUID→URL→hash order) is idempotent; `should_update` logic matches ARCH §11.1.
- [x] Unsubscribe removes only per-user rows; shared content remains.
DoD commit: `feat(db): schema + repositories with shared/per-user dedup model`.

---

## M3 — Cache layer
**Goal:** two-tier cache with correct key separation.
**Refs:** ARCH §10; FND-008.

Tasks:
- [x] `cache::Cache`: moka L1 + optional Redis L2, cache-aside typed values, and bounded per-key TTLs.
- [x] `SCAN`-based prefix invalidation (never `KEYS`) plus versioned Redis Pub/Sub invalidates exact keys and prefixes in sibling instances; sender IDs avoid redundant local work (FND-114).
- [x] Graceful degradation: Redis down ⇒ L1 + source of truth still work.
- [x] Key builders separate shared and per-user namespaces; validated `CACHE_NAMESPACE` values isolate Redis keys and channels between Rusty Feed installations.

Verification: unit tests cover get/set/TTL/local invalidation and unavailable-Redis fallback; Redis integration tests prove L2 round trips, two-instance exact/prefix invalidation, and installation namespace isolation. A disconnected subscriber clears and bypasses L1 until a bounded, successful resubscription.
DoD commit: `feat(cache): moka L1 + redis L2 with SCAN invalidation`.

---

## M4 — Auth & sessions
**Goal:** users can sign in (OAuth + local), sessions + RBAC + CSRF.
**Refs:** ARCH §15, §9; PRD §5.1; FND-110.

Tasks:
- [x] Secure opaque-token sessions backed by PostgreSQL. This deliberate, documented implementation replaces `tower-sessions` (FND-116).
- [x] Local accounts: register/login, Argon2id, open/invite/closed policy, first-persistent-user-admin bootstrap that ignores demo rows, one-use invitation issuance/acceptance, and CLI admin creation.
- [x] Google, GitHub, and Generic OIDC authorization-code flow with PKCE/state plus account linking. Direct `reqwest` is a documented deviation (FND-117).
- [x] RBAC extractor and admin-route guard.
- [x] CSRF protection for cookie-auth mutations and per-IP auth rate limiting.
- [x] Create/list/revoke API tokens as the GReader/REST foundation.
- [x] New users receive the featured default-feed set idempotently without duplicating shared feeds.

Verification: handler tests for register/login/logout, role gating, CSRF rejection, rate limiting.
DoD commit: `feat(web): oauth + local auth, sessions, rbac, csrf`.

---

## M5 — Web shell & reading UI
**Goal:** the server-rendered, progressively enhanced reading experience.
**Refs:** ARCH §9; PRD §5.10, §4.3.

Tasks:
- [x] Askama base layout, embedded local assets, and dependency-light progressive JavaScript. Curated CSS deliberately replaces the proposed Tailwind pipeline (FND-118).
- [x] Theme and typography applied server-side from personal preferences.
- [x] Article list with personal feed/folder/tag/unread filters, date/relevance ordering, and bounded pagination.
- [x] Side-panel, inline, and standalone reading modes, including independent side-panel scrolling.
- [x] Automatic/manual read state, read/unread toggles, bookmarks, and explicit/implicit feedback in personal tables.
- [x] Progressive preferences UI for the implemented display, content, search, media, YouTube, shortcut, and install settings.
- [x] Notification navigation/list/read flows, live counts, polling, and batched feed-health notifications.
- [x] Installable web-app manifest, platform icons, standalone shortcuts, browser-native install UI, and a static-only service-worker cache boundary.

Verification: handler/template tests; snapshot a couple of rendered partials; no-JS smoke (form posts work).
DoD commit: `feat(web): htmx reading UI, reading modes, preferences`.

---

## M6 — Feeds: fetch/parse/dedup + SafeFetch
**Goal:** add feeds and ingest articles correctly and safely (no agents yet — call synchronously or via a manual trigger).
**Refs:** ARCH §11.1, §12; PRD §5.2/§5.3; FND-001/006/011.

Tasks:
- [x] `feeds::SafeFetchClient` with scheme, credential, DNS/private-range, redirect, size, timeout, and implicit-proxy defenses plus SSRF tests.
- [x] Feed URL normalization and shared-feed/personal-subscription dedup.
- [x] `feed-rs` parsing for RSS/Atom/JSON Feed plus media-type-aware content normalization and enclosures.
- [x] Conditional GET sends/stores/reuses `ETag`/`Last-Modified` and handles 304.
- [x] GUID→URL→content-hash article upsert is shared and idempotent.
- [x] Feed add/remove/list/refresh/settings UI with server-rendered endpoints.

Verification: `wiremock` feed server tests (200 with items, 304, redirects, malformed); SSRF unit tests; dedup integration test (N users ⇒ 1 fetch path).
DoD commit: `feat(feeds): safe fetch, parse, conditional GET, dedup ingest`.

---

## M7 — AGENTS framework
**Goal:** the background runtime that guarantees O(1) shared work.
**Refs:** ARCH §8, §7.3; PRD §5.12; FND-007/101.

Tasks:
- [x] `agents::Agent` trait, `AgentContext`, typed durable jobs, registry, and structured outcomes.
- [x] Runtime: durable Postgres `SKIP LOCKED` consumers with bounded, lane-isolated concurrency; user-triggered summaries use reserved low-latency capacity while scheduled/bulk work stays in the background lane. Validated five-field UTC cron expressions control singleton schedules.
- [x] Singleton lease locking for scheduled shared work plus transactional advisory locking where required.
- [x] Run tracking → `agent_runs` (status/duration/stats/trigger) with bounded sanitized errors in the wide, live admin queue view. A per-run `JobLogger` ring buffer remains optional future depth.
- [x] Transactional outbox dispatcher: committed `outbox_events` become deduplicated jobs; unknown deployment-skew events remain pending safely.
- [x] **FeedRefreshAgent** and **FetchFeedAgent** are wired into one deduplicated shared pipeline; downstream enrichment jobs are revision-keyed.
- [x] Standalone `rusty-feed-worker` and `RUN_EMBEDDED_WORKER` server mode.

Verification: test that a feed subscribed by N users enqueues **one** fetch; lock prevents double-run; a failing job is retried/backed-off and recorded.
DoD commit: `feat(agents): scheduler, durable queue, locks, run records, outbox`.

---

## M8 — Content extraction
**Goal:** enrich new articles with full text, safely and politely.
**Refs:** ARCH §12; PRD §5.4; FND-006/104/108/015.

Tasks:
- [x] `extract::Extractor` trait + registry (priority + fallback).
- [x] Local Mozilla-Readability-compatible extraction via `dom_smoothie`, with configurable CSS selection and a density-scored malformed-document fallback.
- [x] Chromium via `chromiumoxide` (separate compose service, semaphore-bounded, `CHROMIUM_ENABLED`): renders a CSP-isolated copy of HTML already fetched through `SafeFetchClient`, then reruns extraction; it never navigates Chromium around the SSRF boundary and degrades to local extraction.
- [x] Per-domain limiter with shared state, robots.txt crawl delay/Disallow, bounded caches, adaptive failure backoff, and `Retry-After` handling.
- [x] Per-subscription RSS/shared-page/isolated-Live default and finite/infinite public-page lifetime; only page mode contributes to the one aggregated shared acquisition. Admin-owned selector/robots/delay/rewrite controls plus separately audited host/path-pinned encrypted acquisition profiles cover explicitly authorized cookie/proxy/certificate cases.
- [x] Media-type-aware feed normalization plus `ammonia` sanitization before store and at render; semantic HTML/Markdown fidelity, lazy/responsive media handling, article-specific typography, and configurable no-network tracking-image removal with shared source-specific reports.
- [x] Bounded no-network YouTube recognition plus a reversible on-demand `youtube-nocookie.com` player with linked-time preservation, explicit connection state, instance/user/subscription policy, and link-only fallback.
- [x] **ExtractContentAgent** (event/access-driven, absent-or-expired public pages only, once per shared article revision) plus RSS fallback and access-triggered fast refresh.

Verification: `wiremock` extraction tests (html, 429+Retry-After, robots crawl-delay); sanitizer XSS tests; SSRF applies to extraction fetches.
DoD commit: `feat(extract): readability + chromium, rate limiting, sanitize`.

---

## M9 — Embeddings & semantic search
**Goal:** one embedding space per instance + fast vector search.
**Refs:** ARCH §13, §11.3; PRD §5.5; FND-002/103/106.

Tasks:
- [x] `embed::Embedder` supports production local BGE-small-en-v1.5 through `fastembed`, explicit online/offline cache preparation, OpenAI-compatible providers, and an explicit deterministic hash fallback for tests/emergencies.
- [x] PostgreSQL vector store with a fixed dimension/model/revision guard, idempotent writes, and HNSW KNN queries.
- [x] Bounded `title\n\nexcerpt\n\ncontent[..2000]` text preparation and batch-capable provider API.
- [x] Semantic and related search use `1 - distance/2`, tenant filters, and configurable hybrid/recency weighting.
- [x] Hybrid keyword via dual weighted `tsvector` GIN indexes, safe web-search operators, explainable score blending, and keyword fallback (P1).
- [x] **EmbedArticleAgent** is revision-deduplicated and supports queued backfill.
- [x] Search UI and versioned native resources provide keyword/semantic/hybrid modes, guarded vectors, pagination, and graceful keyword fallback.

Verification: embedding determinism test (local); model-guard rejects wrong dim (FND-002); search returns expected ordering on a seeded corpus; first-run download smoke (network) + clear error when weights missing offline (FND-103).
DoD commit: `feat(embed,search): single-space embeddings + semantic search`.

---

## M10 — Personalization
**Goal:** per-user learning + relevance ranking.
**Refs:** ARCH §11.2; PRD §5.6.

Tasks:
- [x] TF keyword extraction with the documented stop-word, length, and heuristic constants.
- [x] Explicit ±1.0 and implicit completion/bounce feedback; explicit feedback wins.
- [x] Exact pattern update, decay, prune, and top-100 behavior.
- [x] Sigmoid scoring, neutral fallback, personal cache namespace, and explanations.
- [x] Scheduled and tracked **PatternDecayAgent**.
- [x] Relevance ranking/filtering and **Reset Learning** UI.

Verification: unit tests with exact expected values for extraction/update/decay/prune/sigmoid (regression vs. reference constants).
DoD commit: `feat(search): personalization patterns, feedback, scoring`.

---

## M11 — Saved searches
**Goal:** persistent boolean+semantic queries auto-matching new articles.
**Refs:** ARCH §11.4, §8; PRD §5.7; FND-105 (adjacent).

Tasks:
- [x] Saved-search DSL parser/AST for OR, AND, NOT, phrases, and groups has regression and property-based totality coverage.
- [x] Boolean execution plus documented keyword/semantic score blending.
- [x] Idempotent **SavedSearchScanAgent**/**SavedSearchMatchAgent**, durable matches, threshold notifications, and bounded candidate windows.
- [x] Server-rendered create/edit/list/delete, manual rematch, paginated results, retention controls, notification UI, and native resources.

Verification: parser property tests; matcher integration test (throughput sanity ≥ target); notification on high-relevance.
DoD commit: `feat(search,agents): saved searches DSL + matcher`.

---

## M12 — LLM summarization + cost ledger
**Goal:** optional AI summaries with durable costs.
**Refs:** ARCH §14, §11; PRD §5.8; FND-003/004/013.

Tasks:
- [x] OpenAI/Ollama-compatible clients resolve the enabled user → administrator → environment provider cascade and retry the next profile on provider failure.
- [x] Domain-separated XChaCha20-Poly1305 + HKDF-SHA256 AEAD, masking, versioned AAD, and managed user/admin credential profiles. Delete-and-recreate provides explicit credential rotation without ever redisplaying key material.
- [x] Structured summary/key-points/topics parsing with fallback and first-writer-wins storage on the shared article revision.
- [x] Durable per-user/provider/model/day cost ledger and pricing calculations.
- [x] Low-latency, revision-deduplicated **SummarizeArticleAgent** works on demand and through a live PostgreSQL-backed instance policy. Feed ingestion queues only eligible created/materially updated revisions and post-extraction paths enforce the same configurable normalized plain-text minimum; explicit bounded backfill covers existing articles.
- [x] Live article summary UI, actionable safe failures, and admin cost/queue views.

Verification: `wiremock` LLM tests; AEAD round-trip + tamper-detection tests; first-writer-wins concurrency test; cost math test.
DoD commit: `feat(llm): summarization + durable cost ledger + unified secrets`.

---

## M13 — Admin, OPML, feed health, bulk ops, GReader API
**Goal:** operability + open-standard client interop.
**Refs:** ARCH §9.1, §17; PRD §5.2/§5.9/§5.11/§5.13; FND-105.

Tasks:
- [x] Curated Discover catalog: 200 verified starter feeds, country/topic/language facets, automatic facet filtering, derived feed stats, multi-select subscription, admin publishing/reordering, and unrestricted custom feed URLs.
- [x] Configurable keyboard shortcuts: arrow-key-first defaults, capture/reset controls, server-side validation, and safe Library article/feed actions.
- [x] Personal feed organization: stable hierarchical navigation, one current folder per feed, an Unfiled scope, normalized multi-valued tags, scoped Library/mark-all-read filters, cached source icons, and a persisted reduced sidebar.
- [x] Feed health: structured safe failure history, one-attempt health cycles, daily checks after three failures, weekly checks after 14 failing days, batched notifications, explicit pause/resume, manual-disabled preservation, and reset/queue actions.
- [x] Bulk personal feed operations: folder/Unfiled moves, add/remove/replace tags, queue refresh, pause/resume, and confirmed unsubscribe are tenant-scoped, partially failure-tolerant, and progressively enhanced with drag-to-folder.
- [x] OPML 2.0 file/URL import with bounded dry-run preview, new/existing detection, per-feed and folder-group selection, keep/replace policy, partial result reporting, shared refresh dedup, and hierarchy-preserving export.
- [x] Revocable public article links with encrypted 256-bit bearer tokens, finite/infinite lifetime policy, rotate/revoke, title/metadata/source/image controls, RSS fallback, and anonymous no-store/noindex rendering without visitor-driven work.
- [x] **ContentCleanupAgent** enforces shared public-page lifetimes without deleting RSS content and applies configurable article retention while preserving every user's bookmarks (FND-005/009).
- [x] Admin dashboard: overview/storage counts, user roles, shared-feed inventory/cadence, Discover catalog editing, managed provider configuration, worker/lane/queue visibility, safe wide errors, recent completions, costs, and bounded maintenance actions for refresh, cleanup, stalled recovery, and dead-letter retry.
- [x] **GReader API adapter** exposes ClientLogin + T token, subscription/list+edit, tag/list, stream/contents, items/ids, items/contents, edit-tag, mark-all-as-read, and user-info. Stable numeric aliases are assigned once by database triggers; reads/stars/labels stay personal.
- [x] GReader integration tests replay authentication, subscription, stream, read, and star client request sequences (FND-105).

Verification: admin handler tests; OPML round-trip; health-state transitions; GReader conformance tests (subset).
DoD commit(s): `feat(web): admin, opml, feed health, bulk ops` + `feat(api): greader-compatible client API`.

---

## v1 completion gates

The roadmap was not considered complete merely because the application ran.
All M0–M13 rows and the following cross-cutting implementation/evidence gates
now meet the release-candidate bar:

- [x] Production local ONNX embedding download/offline-seed/reindex workflow.
- [x] Chromium extraction fallback with bounded pooling and graceful degradation.
- [x] GReader and Fever compatibility subsets plus client replay tests; cohesive native API documented.
- [x] Invite, feed-health/bulk/admin, retention, and outbox controls.
- [x] Desktop/tablet/mobile accessibility matrix and strict-CSP completion.
- [x] Performance targets on the documented 100k reference corpus/host.
- [x] Redis/LLM/Chromium/worker failure injection and recovery exercises.
- [x] Upgrade, migration, backup, and restore rehearsal.
- [x] Versioned multi-architecture artifact/container workflow with checksums, SBOM/provenance, and keyless signatures. Actual publication is intentionally tag-triggered and was not performed from this worktree.
- [x] Complete configuration, migration, operations, API, user, and release-evidence references.

The Miniflux compatibility program is a broader release train, not a synonym
for the first Rusty Feed v1. Its 67 independently testable outcomes remain tracked
in [`docs/MINIFLUX_PARITY.md`](MINIFLUX_PARITY.md); 65 are complete and two are
explicitly partial rather than silently overstated.

---

## Post-v1

- **v1.1:** bookmarks, hybrid search, media proxy, public sharing, invitations, and batched health notifications landed in v1. A distinct seen state is recorded when a card is on screen or an article is opened.
- **v2:** configurable keyboard shortcuts, privacy-enhanced and alternative YouTube players, touch gestures, date/number/locale profiles, duration enrichment, Fever compatibility, UI customization, and deeper storage/log administration landed early. Story clustering, daily briefings, ADHD reading format, bare-URL linking, a distinct seen state, password reset, and infinite scroll have since landed. Full translated-string coverage and the remaining turnkey integration adapters remain.

---

## Sequencing & parallelization notes
- **Critical path:** M0→M1→M2 then M6 (feeds) and M7 (agents) unlock the enrichment agents (M8/M9/M11/M12).
- **Parallelizable after M2:** M3 (cache), M4 (auth), M5 (web shell) can proceed alongside M6.
- **M9 before M11** (saved-search matcher uses embeddings). **M6+M7 before M8** (extraction is an agent on ingested articles).
- Keep a running **benchmark harness** from M6 onward to guard PRD §7 targets (FND-101).
- Each milestone must preserve the dedup invariants — add/keep the M2 dedup tests green throughout.
