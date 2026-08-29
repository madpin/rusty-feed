# RustSS — Product Requirements Document

**Status:** Draft for review
**Version:** 0.1.0
**Last updated:** 2026-08-28
**Owners:** Platform team
**Supersedes:** `neureed` (Next.js 16 / TypeScript reference implementation)

---

## 1. Overview

RustSS is a **self-hosted, intelligent RSS/Atom reader** that learns what each user cares about and surfaces it using semantic (vector) search, per-user personalization, and optional LLM summarization. This document specifies the requirements for a **ground-up reimplementation in Rust** of the existing `neureed` product.

The reimplementation exists to deliver the same (and eventually a superset of the) product capabilities with **dramatically lower memory footprint, higher throughput, and simpler operations** — the qualities that matter most for software people run on their own hardware (a NAS, a small VPS, a home server, or a single container).

This is **not** a rewrite for its own sake. The reference implementation is feature-rich and battle-tested; its **data model and algorithms are the source of truth** for behavior. RustSS preserves that behavior while fixing a set of known deficiencies (documented in §9) and re-architecting the runtime for performance and operability.

### 1.1 Product one-liner

> Your RSS reader, but it actually understands what you read — and it runs as a single fast binary you own.

### 1.2 Confirmed product decisions (this cycle)

| Decision | Choice | Rationale |
|---|---|---|
| UI delivery | **Server-rendered HTML + HTMX** (Askama templates, HTMX, Alpine.js, Tailwind) | Lowest memory, fast first paint, all-Rust, progressive enhancement, ideal for self-hosting |
| Infra footprint | **Full**: PostgreSQL + Redis + headless Chromium | Best extraction coverage and multi-instance caching; matches an "own your server" audience willing to run a compose stack |
| Authentication | **OAuth + local accounts** | Google/GitHub/Generic OAuth2 *and* email/password (Argon2), so self-hosters without OAuth apps can still run it |
| v1 milestone | **Feature parity first** | Reproduce current capabilities, then pursue the backlog. Clear success criteria, lower risk |

---

## 2. Goals & Non-Goals

### 2.1 Goals

1. **Behavioral parity** with `neureed` for all shipped features (see §5), validated against documented algorithms and constants.
2. **Multi-user efficiency (critical):** a feed or article subscribed to by *N* users is **fetched, extracted, embedded, and summarized once**, and stored once. Per-user state (read/feedback/scores/subscriptions) is layered on top without duplicating content. (See §6.)
3. **Self-hosted performance:** low idle memory, low per-request latency, high refresh throughput on modest hardware (see §7).
4. **Operational simplicity:** a single primary binary, first-class Docker Compose deployment, safe automatic migrations, graceful degradation when optional dependencies are down.
5. **Correctness & safety:** robust error handling, SSRF protection on *all* outbound fetches, encrypted secrets, and no data-corruption footguns (e.g., mixed-dimension embeddings).
6. **Extensibility:** a clean background-worker ("AGENTS") framework so the backlog (clustering, digests, new agents) can be added without architectural churn.

### 2.2 Non-Goals (v1)

- Reproducing the Next.js SPA's exact micro-interactions 1:1 (the HTMX UI is functionally equivalent, not pixel-identical).
- A public multi-tenant SaaS with billing/quotas (the target is self-hosted, small-to-medium user counts — think 1–500 users per instance).
- A native mobile app (the web UI is responsive/mobile-friendly; native apps are out of scope).
- Real-time collaborative features.
- Migrating the reference implementation's exact encrypted-secret formats (we standardize on one AEAD scheme; a one-time migration path is provided instead — see §9).

---

## 3. Target Users & Personas

| Persona | Description | Primary needs |
|---|---|---|
| **The Self-Hoster (primary)** | Runs the instance for themselves and maybe family/friends. Values privacy, low resource use, easy upgrades. | One-command deploy, low RAM, backups, "it just works" |
| **The Power Reader** | Follows 100s of feeds, wants signal over noise. | Personalized ranking, semantic + saved searches, fast reading, keyboard-driven UX |
| **The Curator / Small Team** | Shares an instance across a handful of users with shared feeds. | Efficient shared fetching, per-user isolation of read state and preferences |
| **The Admin** | Same person as the self-hoster, wearing the ops hat. | Health dashboards, job monitoring, storage/cost visibility, provider config |
| **The Accessibility-Focused Reader** | e.g., ADHD or low-vision users. | Configurable reading formats, typography controls, reduced-motion, keyboard nav |

---

## 4. User Flows

The following flows must be supported end-to-end. Each maps to concrete features in §5.

### 4.1 Onboarding
1. User signs up via OAuth (Google/GitHub/Generic) **or** local email/password.
2. On first login, the user is auto-subscribed to a **curated default feed set** (idempotent; shared feeds created once).
3. User lands on the main reading view with articles already populating.

### 4.2 Add & organize feeds
1. User adds a feed by URL → system validates (SSRF-safe), dedupes by normalized URL (creates the shared feed only if new), subscribes the user.
2. User organizes subscriptions into **personal categories** (collapsible, reorderable, nestable), assigns tags, sets per-feed/per-category overrides (refresh interval, retention, extraction method).
3. User imports/exports **OPML** (with category preservation).

### 4.3 Read & give feedback
1. User browses the article list (filtered by feed/category/unread/search), sorted by date or **personal relevance**.
2. User opens an article in their chosen **reading mode** (side panel / inline / standalone), with configurable typography.
3. Reading is tracked: open → **read**; dwell time → **implicit feedback** (completion vs bounce). Explicit **thumbs up/down** is also supported.
4. The system **learns** (updates per-user keyword patterns) and re-ranks future articles.

### 4.4 Discover
1. **Semantic search:** natural-language query returns meaning-matched articles (optionally recency-biased).
2. **Related articles:** "more like this" from any article's embedding.
3. **Saved searches:** persistent boolean+semantic queries that auto-match new articles and (optionally) notify on high-relevance hits.

### 4.5 Summarize (optional, LLM)
1. User (or an agent) requests a summary → LLM produces summary + key points + topics, stored once on the shared article.
2. Costs are tracked per user and visible to the admin.

### 4.6 Maintain (admin)
1. Admin views system health (DB/Redis/cache, embedding coverage, memory, job history).
2. Admin triggers/monitors background jobs, configures system-wide providers (LLM/embeddings) and constraints, manages users/roles, and inspects storage usage.

---

## 5. Functional Requirements

Requirements are labeled `FR-<area>-<n>`. **P0** = required for v1 parity, **P1** = highly desired in v1 if time permits, **P2** = backlog/future.

### 5.1 Accounts, Auth & Authorization
- **FR-AUTH-1 (P0):** Sign in via Google, GitHub, and a configurable Generic OAuth2 provider.
- **FR-AUTH-2 (P0):** Sign up / sign in via local email + password, hashed with **Argon2id**.
- **FR-AUTH-3 (P0):** Server-side sessions persisted in Postgres; secure, HTTP-only, SameSite cookies.
- **FR-AUTH-4 (P0):** Roles: `ADMIN`, `USER`, `GUEST`. Admin-only routes enforced server-side.
- **FR-AUTH-5 (P0):** New users can discover and explicitly choose curated shared feeds; custom feed URLs always remain available.
- **FR-AUTH-6 (P1):** Registration controls for self-hosters (open / invite-only / closed) and a bootstrap "first user becomes admin" rule.

### 5.2 Feeds & Subscriptions
- **FR-FEED-1 (P0):** Add feed by URL with validation and **SSRF protection** (block localhost, private/link-local IPv4/IPv6, non-http(s)); dedupe by **normalized URL** so the underlying feed is shared across users.
- **FR-FEED-2 (P0):** Parse RSS 2.0/1.0, Atom, and JSON Feed.
- **FR-FEED-3 (P0):** Per-user subscription with `customName`, `tags`, and settings overrides.
- **FR-FEED-4 (P0):** Personal categories: create/rename/delete, **hierarchy (parent/child)**, ordering, collapse state persisted, per-category settings.
- **FR-FEED-5 (P0):** Bulk operations across selected feeds: set category, edit tags (add/remove/replace), update settings, enable/disable, delete, refresh — with **partial-failure tolerance** and per-user authorization checks.
- **FR-FEED-6 (P0):** OPML 2.0 import (auto-create categories, dedupe feeds) and export (grouped by category).
- **FR-FEED-7 (P0):** Feed **health tracking**: states `healthy | warning | error | disabled`, consecutive-failure counting, configurable auto-disable threshold, per-error audit log, manual enable/disable/reset, optional error notifications.
- **FR-FEED-8 (P1):** OPML import with explicit **category selection** and correct category import (fixes a known reference bug).
- **FR-FEED-9 (P0):** Admin-curated feed magazine: editorial title/description, country/category/language facets, featured/order/publish controls, live shared-feed stats (post count, RSS body depth, posting cadence, latest post), and idempotent multi-select subscriptions. Catalog membership never restricts custom URL subscriptions.

### 5.3 Feed Refresh (background)
- **FR-REFRESH-1 (P0):** Scheduled refresh of all due feeds; a feed is refreshed **once per cycle** regardless of how many users subscribe.
- **FR-REFRESH-2 (P0):** Respect an **effective refresh interval** resolved via the settings cascade (feed → category → user → system → env).
- **FR-REFRESH-3 (P0):** **Conditional GET**: send `If-None-Match`/`If-Modified-Since`, store and reuse `ETag`/`Last-Modified`, honor `304 Not Modified` (fixes a known reference gap — see §9).
- **FR-REFRESH-4 (P0):** Article **deduplication** on ingest: GUID (feed-scoped) → URL (global unique) → content hash (SHA-256 of trimmed content, feed-scoped). Update existing article only when content hash, title, or published date meaningfully changed.
- **FR-REFRESH-5 (P0):** Skip disabled/over-error-threshold feeds; record success/failure to health; create per-user refresh notifications with stats.
- **FR-REFRESH-6 (P0):** Bounded concurrency across feeds (configurable).
- **FR-REFRESH-7 (P0):** Manual refresh (single feed, selected feeds, all) via UI/API.

### 5.4 Content Extraction
- **FR-EXTRACT-1 (P0):** Full-text extraction via a **Readability** strategy; **headless-Chromium** strategy for JS-rendered pages (full infra), selected per-feed with a fallback chain.
- **FR-EXTRACT-2 (P0):** Extraction happens **only for new (deduped) articles** and **once per shared article** (never per user).
- **FR-EXTRACT-3 (P0):** Per-**domain** rate limiting with configurable base delay (default 3s), robots.txt `Crawl-delay` support, and adaptive `429` backoff (10s→30s→60s→120s honoring `Retry-After`).
- **FR-EXTRACT-4 (P0):** Shared per-feed content source policy: RSS-only, anonymous public article-page extraction (optional CSS selector, robots/delay/refresh controls), or explicit paywalled/RSS fallback. RustSS does not store publisher credentials, send custom authentication headers, or bypass access controls.
- **FR-EXTRACT-5 (P0):** **SSRF protection on all extraction and robots.txt fetches** (fixes a known reference gap — see §9). HTML output is sanitized before storage/render.
- **FR-EXTRACT-6 (P2):** Rewrite bare-text URLs into links; embed YouTube; media enrichment (backlog).

### 5.5 Embeddings & Semantic Search
- **FR-EMB-1 (P0):** Generate embeddings via **local ONNX model** (BGE-small-en-v1.5, 384-dim) or **OpenAI-compatible API** (e.g., `text-embedding-3-small`, 1536-dim).
- **FR-EMB-2 (P0):** Embeddings are computed once and **stored on the shared article** (accelerating all subscribers).
- **FR-EMB-3 (P0, correctness fix):** **One embedding model per instance.** An instance is configured with a single active embedding model/dimension; all article embeddings belong to that one space, and the stored `(model, dimension)` identity is recorded and enforced. Similarity search only ever compares vectors of the active space. Changing the model requires an explicit re-embed/migration (admin action). This eliminates the reference's mixed-dimension corruption (it zero-pads 384→1536 into one shared column — we must not repeat this). See §9 and Architecture §13.
- **FR-EMB-4 (P0):** Text embedded = title + excerpt + content preview (first ~2000 chars). Batch generation with configurable batch size.
- **FR-EMB-5 (P0):** **Semantic search** via pgvector cosine similarity, normalized to [0,1]; optional **recency-weighted** ranking `final = (1-w)*similarity + w*exp(-age/decay)`; similarity threshold and result limit configurable.
- **FR-EMB-6 (P0):** **Related articles** using a source article's stored embedding.
- **FR-EMB-7 (P1):** Hybrid search (semantic + keyword) with sensible score blending.

### 5.6 Personalization (per-user)
- **FR-PERS-1 (P0):** Learn **per-user keyword patterns** from feedback using TF-based keyword extraction with stop-word filtering.
- **FR-PERS-2 (P0):** **Explicit feedback** (👍 +1.0 / 👎 −1.0) and **implicit feedback** (completion ≥90% → +0.5; bounce < user threshold, default 0.25 → −0.5). Explicit is never overwritten by implicit.
- **FR-PERS-3 (P0):** Pattern weight update `Δw = feedbackValue × relevance × 0.1`; per-30-day **decay** `0.9^floor(days/30)`; prune near-zero weights; keep top **100** patterns by |weight| per user.
- **FR-PERS-4 (P0):** **Relevance scoring** of an article for a user via sigmoid `score = 1/(1+e^(−5·Σ w·relevance))`, neutral 0.5 when no patterns; cached per user with explanations (matched keywords).
- **FR-PERS-5 (P0):** Ranking/filtering by relevance; option to hide or dim low-relevance articles; "Reset Learning" clears patterns.

### 5.7 Saved Searches
- **FR-SS-1 (P0):** Persistent saved searches with a **query DSL**: OR (`,`), required AND (`+`), NOT (`-`), phrases (`"..."`), and groups (`(...)`), parsed to an AST.
- **FR-SS-2 (P0):** Auto-match **new** articles on refresh; score = `0.6·semantic + 0.4·keyword` (title-boosted keyword tf); match iff ≥ threshold (default 0.6).
- **FR-SS-3 (P0):** Optional notifications when a match's relevance ≥ notify threshold (default 0.85); daily-digest flag; recency bias; priority sources.
- **FR-SS-4 (P0):** Interactive execution with pagination; re-match ("rematch") over a recent window (e.g., 90 days); starter **templates**.
- **FR-SS-5 (P0):** Match storage dedupes by `(savedSearch, article)`; indexed for fast retrieval.

### 5.8 LLM Summarization (optional)
- **FR-LLM-1 (P0):** On-demand article **summary + key points + topics**, stored once on the shared article (first-writer-wins).
- **FR-LLM-2 (P0):** Provider cascade: **user config → admin/system config → environment**. Providers: OpenAI-compatible and Ollama.
- **FR-LLM-3 (P0):** Encrypted per-user and system API keys.
- **FR-LLM-4 (P0):** **Cost tracking** (per user, per provider, daily/monthly) that is **durable** (persisted, not in-process only — fixes a known reference gap). Admin cost visibility.
- **FR-LLM-5 (P1):** Optional auto-summarization on refresh, gated by admin + cascade settings and a minimum content length.
- **FR-LLM-6 (P2):** **News clustering + cluster digests** (LLM summary of an event across articles, with source links) — backlog.

### 5.9 Notifications
- **FR-NOTIF-1 (P0):** In-app notifications (feed refresh stats, feed health error/disabled, general info/warn/error/success), stored per user, unread counts, mark read / mark all read, retention (keep last 100/user).
- **FR-NOTIF-2 (P1):** Batched health notifications (max 1 per feed per hour).

### 5.10 Reading Experience & Preferences
- **FR-UX-1 (P0):** Three reading modes: **side panel** (resizable), **inline** (accordion), **standalone** (full page).
- **FR-UX-2 (P0):** Extensive user preferences (30+ fields): theme (14 themes incl. system), font sizes, reading typography (family, size, line-height, paragraph/break spacing), article list density/borders/spacing/section order, sidebar width/collapse, articles-per-page, infinite-scroll mode, sort order/direction, show/hide read, auto-mark-read, related excerpts, reading-time display, learning knobs (bounce threshold, recency weight/decay, low-relevance visibility), and per-user LLM/embedding config.
- **FR-UX-3 (P0):** Preferences applied server-side to rendered pages (theme/typography) and persisted.
- **FR-UX-4 (P1):** "Seen" state (card viewed) distinct from "read".
- **FR-UX-5 (P0):** Configurable keyboard shortcuts with server-side conflict validation, safe editable-control/IME/modifier guards, and an easy preference capture/reset UI. Default commands cover next/previous article, save for later, mark unread, and next/previous feed.
- **FR-UX-6 (P2):** Touch gestures, configurable date formats, ADHD-friendly reading formatting, and i18n/language switcher — backlog.

### 5.11 Admin & Operations
- **FR-ADM-1 (P0):** Admin dashboard: system stats for users, shared feeds, articles, catalog entries, active jobs, and durable AI costs; links to management surfaces.
- **FR-ADM-2 (P0):** Job management: view **run history** (status, duration, stats, logs), trigger jobs manually.
- **FR-ADM-3 (P0):** System configuration: embedding provider + auto-generate, LLM system credentials/models, provider enable/disable, new-user default preferences, and **min/max constraints** for user settings.
- **FR-ADM-4 (P0):** User management: list users and change admin/user roles with CSRF protection, self-demotion prevention, and last-admin protection. Resetting another user's learned data remains backlog.
- **FR-ADM-5 (P0):** Maintenance actions: cache clear, Postgres `VACUUM/ANALYZE`, delete/regenerate embeddings, cleanup old articles.
- **FR-ADM-6 (P1):** Storage/cost visibility with cleanup/compaction actions; installation/version info; application log viewer (backlog-aligned).

### 5.12 Background Jobs / Agents
- **FR-JOB-1 (P0):** Scheduled agents: **feed refresh** and **cleanup** (retention) run on cron; **pattern decay**, **embedding backfill**, and **saved-search matching** are available as scheduled and/or on-demand agents.
- **FR-JOB-2 (P0):** **Distributed singleton locking** so a job runs on only one instance at a time (lease with timeout), plus durable **run records** (status/duration/stats/logs).
- **FR-JOB-3 (P0):** Cleanup honors retention (max age, max articles/feed) resolved via the settings cascade and **preserves favorited/bookmarked** articles (implements the reference's intended-but-unimplemented `preserveStarred`).
- **FR-JOB-4 (P1):** Bookmarks/favorites as a first-class per-user model (prerequisite for FR-JOB-3's preservation and a backlog item).

### 5.13 API
- **FR-API-1 (P0):** A cohesive HTTP API mirroring the reference's resource surface (articles, feeds, user data, saved searches, admin, jobs, health) for automation, non-browser clients, and cron/webhook triggers.
- **FR-API-2 (P0):** Consistent JSON envelopes (`{ data, message }` / `{ error, details }`) and status codes (200/400/401/403/404/409/429/500). Request validation on all mutating endpoints.
- **FR-API-3 (P0):** `/api/health` liveness/readiness endpoint (checks DB, and reports optional-dependency status).
- **FR-API-4 (P1):** A safe **proxy** endpoint for images/media to avoid mixed-content and leaking the reader's IP, with SSRF protection.
- **FR-API-5 (P1) — Open-standard client API (GReader):** Expose a **Google Reader API (GReader)-compatible** endpoint surface so existing third-party RSS clients (Reeder, FeedMe, FocusReader, NetNewsWire, etc.) can connect out of the box. RustSS's own API can be new/clean; the GReader layer is an adapter on top of the same services. Scope: `ClientLogin` token auth + write token (`T`), `subscription/list` & `subscription/edit`, `tag/list`, `stream/contents` & `stream/items/ids` & `stream/items/contents`, `edit-tag` (read/star/like state), `mark-all-as-read`, and `user-info`. Reads map to shared articles; read/star/tag state maps to the per-user tables. (See Architecture §9.1.) *Rationale: interoperability with the established self-hosted ecosystem, as used by FreshRSS/Miniflux.*
- **FR-API-6 (P2):** Additional client-compat layer (Fever API) if demand warrants — backlog.

---

## 6. The Multi-User Deduplication Requirement (Critical)

This is the defining architectural constraint and is called out separately because it governs the schema, caching, and agent design.

**Principle:** *Content is shared; state is personal.*

| Shared once, system-wide | Personal, per user |
|---|---|
| `feed` (unique by normalized URL) | subscription (`user_feed`: custom name, tags, overrides) |
| `article` (unique by URL; feed-scoped GUID/hash) | read status (`read_article`) |
| article **embedding** (per model/space) | feedback (`article_feedback`) |
| article **summary / key points / topics** | learned patterns (`user_pattern`) |
| feed **health** & error log | relevance **scores** (cache, derived) |
| domain rate-limit state | saved searches & their matches |
| | preferences, categories, notifications |

**Hard requirements:**
- **FR-DEDUP-1:** Adding a feed that already exists (by normalized URL) MUST NOT create a second feed row; it only creates the subscription.
- **FR-DEDUP-2:** An article present in multiple subscribed feeds/users MUST exist as a single row (URL is globally unique).
- **FR-DEDUP-3:** Fetching, extraction, embedding, and summarization for a shared article/feed MUST execute **at most once** per content version, not once per subscriber. Concurrent workers MUST coordinate (locks/idempotent upserts) to avoid duplicate work.
- **FR-DEDUP-4:** Per-user reads/feedback/scores MUST be isolated and MUST NOT leak across users.
- **FR-DEDUP-5:** Deleting a subscription MUST NOT delete shared content still referenced by others; shared content is reclaimed only by retention/cleanup policy.
- **FR-DEDUP-6:** Caching keys MUST separate shared derivations (keyed by article/feed) from per-user derivations (keyed by user+entity) to maximize cross-user cache reuse.

**Success metric:** For a feed shared by *N* users, network fetches, extraction runs, embedding computations, and summary generations per refresh cycle are **O(1)**, not **O(N)**.

---

## 7. Non-Functional Requirements

### 7.1 Performance & footprint (self-hosted targets)

Targets assume a modest host (≈2 vCPU / 2 GB RAM) with Postgres+Redis local, unless noted. These are **product targets** to design and benchmark against, not guarantees.

| Metric | Target |
|---|---|
| Idle RSS (app process, excl. Postgres/Redis/Chrome) | **< 100 MB** |
| Loaded RSS (typical small instance, 5–20 users) | **< 300 MB** |
| Article-list page render (server, cache-warm, p95) | **< 50 ms** |
| Article-list page render (cache-cold, p95) | **< 200 ms** |
| Semantic search (p95, ≤100k articles, HNSW) | **< 150 ms** |
| Relevance score (cache-warm, p95) | **< 5 ms** |
| API JSON endpoint (p95, non-search) | **< 75 ms** |
| Feed refresh throughput | **≥ 200 feeds/min** on 2 vCPU (fetch-bound), extraction gated by per-domain limits |
| Saved-search matching | **≥ 1000 articles/min** |
| Cold start to serving | **< 2 s** (excl. first local-model load) |
| Container image size (app) | **< 150 MB** (excl. Chromium layer) |

### 7.2 Scalability
- Correct behavior for **1–500 users** and **up to ~1M articles** on a single node.
- **Horizontal scale:** multiple stateless app instances behind a load balancer share Postgres + Redis; background agents coordinate via distributed locks and a shared queue so exactly-one-runs semantics hold.

### 7.3 Reliability & availability
- **Graceful degradation:** if Redis is down, serve from L1 (in-process) cache / DB; if the LLM/embedding provider is down, core reading/search still works; if Chromium is unavailable, fall back to Readability.
- **Durable jobs:** job runs and queue items survive restarts; interrupted singleton jobs are re-acquirable after lease expiry.
- **At-least-once** background processing with **idempotent** writes (safe to retry).

### 7.4 Security & privacy
- SSRF protection on **every** outbound fetch (feeds, extraction, robots.txt, media proxy), including private-range and link-local blocking; scheme allow-list.
- Stored LLM/API credentials encrypted at rest with a single authenticated-encryption scheme; secret material never logged. Public article-page extraction never uses publisher cookies or credentials.
- Output sanitization of all stored/rendered feed HTML (XSS-safe).
- Standard web hardening: CSRF protection for cookie-authenticated mutations, secure cookies, security headers, per-IP/user rate limiting on auth and expensive endpoints.
- Least-privilege: non-root container, read-only FS where possible.

### 7.5 Observability
- Structured logging (levels, spans) via `tracing`; request tracing with correlation IDs.
- Prometheus-compatible metrics (request latency/count, refresh stats, queue depth, cache hit-rate, embedding/LLM cost counters).
- Health/readiness endpoints; admin-visible job history and system stats.

### 7.6 Maintainability & quality
- Idiomatic, modular Rust; `thiserror` for domain errors, `anyhow` at binary boundaries.
- Compile-time-checked SQL; migrations versioned and applied safely on startup (opt-in).
- Test pyramid: unit tests for algorithms (scoring, decay, dedup, query parser), integration tests against ephemeral Postgres, and HTTP-level tests. CI gates on fmt/clippy/test.

### 7.7 Portability & operations
- Runs on Linux (x86_64 + arm64). Single primary binary + assets.
- First-class **Docker Compose** (app + Postgres/pgvector + Redis + headless Chrome) and multi-arch images.
- Config via environment (typed, validated at startup) with sane defaults; `.env` support for local dev.
- Safe automatic DB migrations; documented backup/restore.

### 7.8 Accessibility & i18n
- WCAG-minded UI: keyboard navigable, ARIA on interactive components, reduced-motion support, sufficient contrast across themes.
- i18n-ready structure (message catalogs) even if only English ships in v1.

---

## 8. Feature Scope by Milestone

### 8.1 v1 — Parity (P0)
All P0 requirements in §5, the multi-user dedup guarantees in §6, and the NFR targets in §7. Concretely: accounts (OAuth + local), feeds/subscriptions/categories/OPML/health/bulk-ops, refresh with conditional GET + dedup, extraction (Readability + Chromium) with rate limiting + SSRF, embeddings (local + OpenAI, **one model per instance** with a safe fixed-dimension store), semantic/related search, personalization, saved searches, LLM summarization with **durable cost tracking**, notifications, full reading UI + preferences, admin dashboard, background agents with locking + run history, and the HTTP API.

### 8.2 v1.1 — Parity+ (P1)
Registration controls; OPML category selection/fix; hybrid search; "seen" state; batched health notifications; **bookmarks/favorites** + retention preservation; storage/cost admin views; media proxy.

### 8.3 v2 — Backlog (P2)
News **clustering + LLM cluster digests**; ADHD-friendly reading formatting; **i18n**/language switcher; touch gestures; configurable date formats; auto-link bare URLs; YouTube embeds; installation/version + log viewer; advanced OPML.

---

## 9. Known Reference-Implementation Deficiencies to Fix

RustSS explicitly corrects these behaviors discovered during analysis of `neureed`:

1. **Conditional GET not wired into refresh.** ETag/Last-Modified columns exist but are never populated/used → every refresh does a full GET. **Fix:** implement conditional GET end-to-end (FR-REFRESH-3).
2. **Mixed-dimension embedding corruption.** Local (384-dim) vectors are zero-padded into the same `vector(1536)` column as OpenAI vectors, silently breaking similarity when providers are mixed across articles. **Fix:** model/space-tagged embedding storage (FR-EMB-3).
3. **In-memory cost tracking.** Embedding and summarization costs live in process memory and are lost on restart / not shared across instances. **Fix:** durable, per-user cost accounting (FR-LLM-4).
4. **Two incompatible encryption schemes.** LLM keys and feed cookies use different key derivation *and* ciphertext field ordering. **Fix:** one AEAD scheme + a one-time migration.
5. **`preserveStarred` is a no-op.** Cleanup references starred articles, but no bookmark model exists. **Fix:** real bookmarks/favorites model honored by retention (FR-JOB-3/4).
6. **SSRF gaps in extraction/robots fetches.** Only feed creation validates URLs; extraction and robots.txt fetching lack private-range guards. **Fix:** centralized SSRF-safe fetch client (FR-EXTRACT-5).
7. **Unstarted/duplicated job wiring.** Embedding-generation cron is defined but never started; pattern-decay lacks locking/run-tracking. **Fix:** all agents run through one framework with locking + run records (FR-JOB-1/2).
8. **`KEYS`-based cache invalidation** (O(N)) → use `SCAN`/tag-based invalidation.
9. **Cleanup ages by `createdAt`, not `publishedAt`** (surprising for backfilled feeds) → make the age basis explicit/configurable.

---

## 10. Assumptions & Constraints

- Single logical Postgres (with pgvector) is the system of record. Redis is a cache/coordination layer, not a source of truth.
- **One embedding model per instance** (confirmed): all embeddings live in a single vector space; switching models is an explicit admin re-embed. This keeps vector storage and search simple and correct.
- **Local embedding model artifacts are downloaded once on first run** (confirmed) and cached in a persistent volume (keeps the image small). First inference incurs a model-load cost (excluded from the cold-start target); first run needs network access to fetch weights.
- Headless Chromium is provided by the compose stack; extraction gracefully falls back to Readability if it is unavailable.
- Target user counts are small-to-medium (self-hosted), enabling design choices (e.g., Postgres-backed queue) that would differ at SaaS scale.
- Behavior/algorithm constants default to the reference values (documented in the Architecture doc) but are configurable.

---

## 11. Success Metrics

- **Parity:** 100% of P0 features demonstrably working; algorithm outputs match reference within tolerance on a shared test corpus.
- **Dedup efficiency:** verified O(1) shared work for N-subscriber feeds (fetch/extract/embed/summarize counters).
- **Footprint:** idle and loaded RSS within §7.1 targets on the reference host.
- **Latency:** search/list/API p95s within §7.1 targets on a 100k-article corpus.
- **Operability:** one-command compose bring-up; automatic migrations; clean graceful-degradation behavior in fault-injection tests.

---

## 12. Decisions & Open Questions

### 12.1 Resolved (this cycle)
1. **Product name:** ✅ **RustSS** (repo `rustss`).
2. **Local model distribution:** ✅ **Download on first run**, cached in a persistent volume (smaller image; first run needs network).
3. **Embedding policy:** ✅ **One embedding model per instance** (single vector space; model change ⇒ explicit re-embed).
4. **Client API compatibility:** ✅ A **new, clean native API** *plus* a **Google Reader (GReader)-compatible** adapter for open-standard client interoperability (FR-API-5).

### 12.2 Still open
1. **Digest delivery (v2):** in-app only, or also email? (Affects whether we add an SMTP dependency.)
2. **Local model choice/default:** confirm BGE-small-en-v1.5 (384-dim) as the shipped default vs. an alternative (e.g., `all-MiniLM-L6-v2`); both are fastembed-supported.
3. **GReader auth mapping:** map GReader `ClientLogin` to local-account credentials only, or also mint app-specific tokens for OAuth users? (Leaning app-specific tokens.)

---

*Next documents:* see `docs/ARCHITECTURE.md` (system design, data model, tech stack, AGENTS framework) and `README.md` (project guide).
