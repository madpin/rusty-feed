# RustSS — Architecture Document

**Status:** Draft for review
**Version:** 0.1.0
**Last updated:** 2026-08-28
**Companion docs:** `docs/PRD.md` (requirements), `README.md` (project guide)

---

## Table of Contents

1. [Purpose & Scope](#1-purpose--scope)
2. [Architectural Principles](#2-architectural-principles)
3. [Proposed Tech Stack](#3-proposed-tech-stack)
4. [System Overview](#4-system-overview)
5. [Workspace & Module Structure](#5-workspace--module-structure)
6. [Data Model](#6-data-model)
7. [Multi-User Deduplication Design (Critical)](#7-multi-user-deduplication-design-critical)
8. [The AGENTS Framework (Agentic Background Workflows)](#8-the-agents-framework-agentic-background-workflows)
9. [Request Lifecycle & Web Layer](#9-request-lifecycle--web-layer)
10. [Caching Strategy](#10-caching-strategy)
11. [Core Algorithms & Constants](#11-core-algorithms--constants)
12. [Content Extraction & Safe Fetch](#12-content-extraction--safe-fetch)
13. [Embeddings, Semantic Search & Vector Storage](#13-embeddings-semantic-search--vector-storage)
14. [LLM Integration & Secrets](#14-llm-integration--secrets)
15. [Authentication & Authorization](#15-authentication--authorization)
16. [Configuration](#16-configuration)
17. [Observability & Operations](#17-observability--operations)
18. [Deployment Topology](#18-deployment-topology)
19. [Error Handling Strategy](#19-error-handling-strategy)
20. [Testing Strategy](#20-testing-strategy)
21. [Migration from the Reference Implementation](#21-migration-from-the-reference-implementation)
22. [Key Design Decisions (ADR summary)](#22-key-design-decisions-adr-summary)

---

## 1. Purpose & Scope

This document defines the technical architecture for RustSS — a self-hosted, multi-user, AI-augmented RSS reader. It proposes a **complete tech stack**, the **data model**, the **background-agent framework**, and the strategies (caching, dedup, security, deployment) needed to satisfy the PRD, with special emphasis on the **multi-user content-deduplication** guarantee and **self-hosted performance/footprint**.

Behavioral parity with the reference is the baseline; §21 and RustSS explicitly corrects these behaviors discovered during analysis of `neureed`.

---

## 2. Architectural Principles

1. **Content is shared; state is personal.** The schema and workers are organized so shared content (feeds, articles, embeddings, summaries) is stored and computed once, and per-user state is layered on top. This is the north star (see §7).
2. **Do expensive work once, lazily, and idempotently.** Fetch/extract/embed/summarize are claimed per shared entity, safe to retry, and never fan out per subscriber.
3. **Small, composable, testable modules.** Domain logic lives in libraries with narrow traits ("ports"); I/O adapters (Postgres, Redis, HTTP, ONNX) are swappable and mockable.
4. **Fast by default, low memory always.** Async I/O (Tokio), zero-copy where practical, compile-time SQL and templates, bounded concurrency, streaming over buffering.
5. **Degrade gracefully.** Optional dependencies (Redis, LLM, Chromium) failing must not take down core reading/search.
6. **Secure by construction.** Every outbound fetch is SSRF-guarded; every rendered HTML is sanitized; every secret is encrypted; every mutation is validated and CSRF-protected.
7. **Boring, durable operations.** One primary binary, Postgres as system of record, automatic-but-safe migrations, first-class Compose.

---

## 3. Proposed Tech Stack

> Versions are pinned to recent, vetted releases in `Cargo.toml` at implementation time (no floating/`*` ranges). The table records **choices and rationale**, and one primary + fallback per concern.

### 3.1 Runtime & Web

| Concern | Choice | Alternative | Why |
|---|---|---|---|
| Language / runtime | **Rust + Tokio** (async) | — | Performance, memory safety, low footprint |
| Web framework | **Axum** (+ `tower`, `tower-http`) | Actix-web, Poem | Tower middleware ecosystem, hyper-based perf, ergonomic extractors, first-class Tokio |
| Templating (UI) | **Askama** (compile-time, type-safe) | Maud, Minijinja | Zero-runtime-cost templates, compile-time checking, Jinja-like ergonomics |
| Interactivity | **HTMX + Alpine.js** | — | Server-driven UI, progressive enhancement, tiny client JS |
| Styling | **Tailwind CSS** (standalone CLI at build) | — | No Node runtime dependency; utility CSS matches the reference's design system |
| Static assets | **`rust-embed`** (embed CSS/JS/templates) | tower-http `ServeDir` | Single-binary distribution; assets baked in |
| CSRF | **double-submit token middleware** (`axum-csrf`) | custom | Cookie-auth mutation protection |
| Rate limiting | **`tower_governor`** | custom | Per-IP/route throttling for auth & expensive endpoints |

### 3.2 Data & Cache

| Concern | Choice | Alternative | Why |
|---|---|---|---|
| Database | **PostgreSQL 16 + pgvector** | — | System of record; vector search; the reference's data model is Postgres-native |
| DB access | **SQLx** (async; runtime queries + `FromRow`) | SeaORM, Diesel | Raw-SQL friendliness for pgvector, no heavy ORM, low overhead. We use runtime `query`/`query_as` (not `query!`) so builds are hermetic; SQL is validated by integration tests against real Postgres (FND-107) |
| Vector types | **`pgvector`** crate (SQLx integration) | raw text casts | Typed `Vector` binding for `vector` columns |
| Migrations | **`sqlx::migrate!`** (embedded) | refinery | Versioned SQL migrations applied safely on startup |
| L1 cache | **`moka`** (in-process, async, TTL/LRU) | — | Always-on, zero-dependency hot cache; keeps Redis load down |
| L2 cache / coordination | **`fred`** (Redis client, pooled, pub/sub) | `redis` + `deadpool` | Shared cache + cross-instance invalidation + locks |
| IDs | **UUIDv7** (`uuid`) | ULID | Time-sortable PKs, index-friendly, globally unique |
| Time | **`time`** | chrono | Lightweight, `timestamptz` support |
| Hashing | **`sha2`** (content hash), `blake3` (fast digests) | — | SHA-256 parity for content hash; BLAKE3 for internal keys |

### 3.3 Background Work & Scheduling

| Concern | Choice | Alternative | Why |
|---|---|---|---|
| Job framework | **`apalis`** (workers + cron), **Postgres storage** | custom `SKIP LOCKED` queue | Durable queue, cron scheduling, retries/backoff; Postgres backend avoids extra infra and keeps jobs queryable |
| Singleton locks | **Postgres advisory locks** + `agent_lock` lease table | Redis `SET NX PX` | Exactly-one-runner semantics across instances; DB is authoritative |
| Event fan-out | **Redis pub/sub** (optional) + DB outbox | — | Low-latency "new articles" notification to worker(s); DB outbox for durability |

> Rationale: with "Full infra" chosen, Redis is available; however we keep the **queue in Postgres** so job state is durable, transactional with content writes (outbox pattern), and survives Redis outages. Redis is used for caching, ephemeral coordination, and low-latency event nudges.

### 3.4 Feeds, Extraction, AI

| Concern | Choice | Alternative | Why |
|---|---|---|---|
| HTTP client | **`reqwest`** (rustls, gzip/br, timeouts) | hyper directly | Ergonomic, TLS via rustls (no OpenSSL), compression |
| Feed parsing | **`feed-rs`** | custom | RSS 1.0/2.0, Atom, JSON Feed in one crate |
| HTML parsing | **`scraper`/`html5ever`** | lol_html | Robust DOM for extraction & sanitization |
| Readability extraction | **`article_scraper`** (Readability + site configs) | `dom_smoothie`, `readability` | Proven in an RSS-reader context; configurable |
| Headless extraction | **`chromiumoxide`** (CDP, async) | fantoccini | JS-rendered pages; async CDP; pooled browser |
| HTML sanitization | **`ammonia`** | — | XSS-safe cleaning of stored/rendered feed HTML |
| Local embeddings | **`fastembed`** (ONNX via `ort`) | candle | BGE-small-en-v1.5 (384-dim) parity; CPU-friendly |
| ONNX runtime | **`ort`** | — | Backs fastembed; CPU inference |
| LLM / OpenAI | **`async-openai`** (OpenAI + compatible base URLs) | raw reqwest | Covers OpenAI, Azure, and Ollama's `/v1`; typed |
| Ollama (native) | **`async-openai` via `/v1`** (fallback reqwest) | `ollama-rs` | Reuse one client; native API only if needed |
| Robots.txt | small parser (crawl-delay/allow) | `texting_robots` | Honor crawl-delay; cache per domain |

### 3.5 Auth, Validation, Crypto

| Concern | Choice | Alternative | Why |
|---|---|---|---|
| OAuth2 | **`oauth2`** | openidconnect | Google/GitHub/Generic OAuth2 flows |
| Sessions | **`tower-sessions`** + `tower-sessions-sqlx-store` (Postgres) | axum-login | Server-side sessions in Postgres (matches reference DB-session model) |
| Password hashing | **`argon2`** (Argon2id) | — | Local accounts, memory-hard hashing |
| Input validation | **`garde`** (or `validator`) + `serde` | — | Zod-equivalent runtime validation on DTOs |
| (De)serialization | **`serde` / `serde_json`** | — | Standard |
| Secret encryption | **XChaCha20-Poly1305** (`chacha20poly1305`) + **HKDF-SHA256** key derivation | `aes-gcm` | One AEAD scheme for API keys & cookies; 192-bit nonce is misuse-resistant |

### 3.6 Config, Errors, Observability, Testing

| Concern | Choice | Alternative | Why |
|---|---|---|---|
| Config | **`figment`** (env + file layers) + `dotenvy` | `config` | Typed, layered, validated at startup |
| Errors | **`thiserror`** (domain) + **`anyhow`** (bins) | — | As required by the PRD; clean error boundaries |
| Logging/tracing | **`tracing`** + `tracing-subscriber` + `tower-http` `TraceLayer` | — | Structured logs, spans, correlation IDs |
| Metrics | **`metrics`** + `metrics-exporter-prometheus` | — | Prometheus scrape endpoint |
| Tracing export (opt) | **`tracing-opentelemetry`** + `opentelemetry-otlp` | — | Optional distributed tracing |
| HTTP mocking (tests) | **`wiremock`** | — | Deterministic feed/LLM tests |
| DB tests | **`sqlx::test`** + ephemeral Postgres (testcontainers) | — | Real Postgres/pgvector behavior |
| Lint/format | **`clippy` + `rustfmt`** (CI-gated) | — | Consistency |

---

## 4. System Overview

```
                                  ┌────────────────────────────────────────────────┐
                                  │                  Clients                         │
                                  │   Browser (HTMX/Alpine)   •   API consumers      │
                                  └───────────────┬───────────────────┬─────────────┘
                                                  │ HTML over HTTP     │ JSON over HTTP
                                                  ▼                    ▼
        ┌──────────────────────────────────────────────────────────────────────────────────┐
        │                             rustss-server (Axum)                                  │
        │                                                                                    │
        │  tower-http (trace, compression, timeout, CORS) → auth/session → CSRF → routers    │
        │                                                                                    │
        │   ┌────────────┐   ┌────────────┐   ┌──────────────┐   ┌───────────────────────┐  │
        │   │ HTML pages │   │  JSON API  │   │  Auth (OAuth/ │   │  Admin dashboard      │  │
        │   │ (Askama)   │   │  handlers  │   │  local+2FA?)  │   │  (stats/jobs/config)  │  │
        │   └─────┬──────┘   └─────┬──────┘   └──────┬───────┘   └──────────┬────────────┘  │
        │         └─────────────────┴─── application services (domain) ─────┘               │
        │                                     │                                              │
        └─────────────────────────────────────┼──────────────────────────────────────────────┘
                     │                          │                          │
                     ▼                          ▼                          ▼
             ┌───────────────┐         ┌────────────────┐        ┌──────────────────┐
             │  Postgres 16  │         │     Redis      │        │  Object/Model     │
             │  + pgvector   │◄──────► │ (L2 cache,     │        │  store: local     │
             │ (system of    │  outbox │  pub/sub,      │        │  ONNX weights,    │
             │  record +     │  events │  ephemeral)    │        │  embedded assets  │
             │  job queue)   │         └────────────────┘        └──────────────────┘
             └──────┬────────┘
                    │ SKIP LOCKED / apalis
                    ▼
        ┌──────────────────────────────────────────────────────────────────────────────────┐
        │                     rustss-worker (AGENTS runtime)                                │
        │  Scheduler (cron) → enqueue │ Queue consumers (bounded) │ Singleton locks (advisory)│
        │                                                                                    │
        │  FeedRefresh → FetchFeed → ExtractContent → EmbedArticle → SavedSearchMatch        │
        │  Summarize (on-demand) • PatternDecay • Cleanup • Health • (v2) Cluster/Digest      │
        │                                                                                    │
        │  Outbound via SafeFetchClient (SSRF-guarded)  •  Chromium pool  •  Providers       │
        └──────────────────────────────────────────────────────────────────────────────────┘
```

The **server** and **worker** are the same codebase and can run **in one process** (default, self-hosted) or as **separate deployments** (horizontal scale). Both share the domain libraries; the worker hosts the AGENTS runtime, the server can optionally embed it (`RUN_EMBEDDED_WORKER=true`).

---

## 5. Workspace & Module Structure

A Cargo **workspace** with small, focused crates. Domain crates expose traits; adapters implement them. This keeps compile times reasonable, enables mocking, and enforces layering.

```
rustss/
├─ Cargo.toml                 # workspace
├─ crates/
│  ├─ core/                   # domain types, errors, config, port traits (no I/O)
│  │   ├─ models/             #   User, Feed, Article, Subscription, Category, ...
│  │   ├─ ports/              #   traits: Repositories, Cache, VectorStore, Embedder,
│  │   │                      #           LlmClient, FetchClient, Extractor, Queue, Clock
│  │   └─ error.rs            #   thiserror domain errors
│  ├─ db/                     # SQLx repositories + migrations + pgvector adapters
│  │   ├─ migrations/         #   *.sql (sqlx::migrate!)
│  │   └─ repos/              #   FeedRepo, ArticleRepo, UserRepo, VectorStore(pg), ...
│  ├─ cache/                  # moka (L1) + fred/Redis (L2) implementing Cache port
│  ├─ feeds/                  # feed fetch (SafeFetchClient) + parse (feed-rs) + dedup
│  ├─ extract/               # Readability (article_scraper) + Chromium (chromiumoxide)
│  │                          #   + per-domain rate limiter + robots.txt + sanitizer
│  ├─ embed/                  # local (fastembed/ort) + OpenAI-compatible embedders
│  ├─ llm/                    # OpenAI/Ollama chat clients + summarization + cost meter
│  ├─ search/                 # semantic search, saved-search DSL/AST/matcher, scoring,
│  │                          #   pattern detection (TF), personalization
│  ├─ agents/                 # AGENTS framework + concrete agents (uses all above)
│  └─ web/                    # Axum app: routers, handlers, Askama templates, auth,
│                             #   HTMX partials, DTOs (garde-validated), error mapping
├─ bins/
│  ├─ server/                 # main HTTP server (optionally embeds worker)
│  ├─ worker/                 # standalone AGENTS runtime
│  └─ cli/                    # admin/maintenance: migrate, create-admin, reindex, backfill
├─ assets/                    # tailwind input, compiled css/js, htmx, alpine (embedded)
├─ templates/                 # Askama templates (.html)
├─ docs/                      # PRD.md, ARCHITECTURE.md, ...
├─ docker/                    # Dockerfile(s), compose, entrypoint
└─ AGENTS.md                  # build/test/run commands & conventions (for contributors/agents)
```

**Layering rule:** `core` depends on nothing app-specific; `db/cache/feeds/extract/embed/llm/search` depend on `core`; `agents` and `web` depend on the domain crates; `bins` wire everything together (dependency injection at startup). No crate reaches "sideways" into another adapter's internals.

---

## 6. Data Model

PostgreSQL is the system of record. The model preserves the reference's shared/per-user split (§7) and adds fixes: **space-tagged embeddings**, **bookmarks/favorites**, **seen state**, **durable cost ledger**, **unified encrypted secrets**, and a clean **agent run/queue/lock** set. PKs are **UUIDv7** (`created_at` derivable/sortable). All timestamps are `timestamptz`.

### 6.1 Entity groups (ER summary)

```
Accounts/Auth
  users (id, email UNIQUE, name, image, role[ADMIN|USER|GUEST], password_hash?, email_verified_at?, created_at, updated_at)
  oauth_accounts (id, user_id→users, provider, provider_account_id, tokens..., UNIQUE(provider, provider_account_id))
  sessions (id, user_id→users, expires_at, data)      -- tower-sessions store
  local_credentials (optional split of password data)  -- or inline on users
  api_tokens (id, user_id→users, name, token_hash UNIQUE, scopes, last_used_at?, created_at, revoked_at?)  -- app-specific tokens (GReader/native API)

Feeds (SHARED)
  feeds (id, url UNIQUE(normalized), name, site_url, description, image_url,
         etag?, last_modified?, last_fetched?, last_successful_fetch?,
         fetch_interval_default, health_status[healthy|warning|error|disabled],
         consecutive_failures, error_count, auto_disable_threshold, notify_on_error,
         http_status?, redirect_url?, extraction_settings jsonb,  -- shared mode/selector policy
         respect_robots_txt, extraction_delay_ms?, created_at, updated_at)
  feed_error_log (id, feed_id→feeds CASCADE, ts, error_type, error_message, http_status?, details jsonb, resolved)
  feed_icons (feed_id→feeds CASCADE PK, source_url, canonical raster MIME + bounded BYTEA, attempt/fetch/error timestamps)
  feed_catalog_entries (id, feed_id→feeds UNIQUE, title, description?, category, country, language,
                        featured, sort_order, published, created_at, updated_at)
  domain_extraction_state (domain UNIQUE, last_extracted_at, failure_count, rate_limit_count, crawl_delay_ms?, last_error?)

Articles (SHARED)
  articles (id, feed_id→feeds CASCADE, url UNIQUE, guid?, title, author?, content, excerpt?,
            content_hash, image_url?, published_at?, extraction_status[NONE|SUCCESS|FAILED], extraction_error?,
            summary?, key_points jsonb?, topics text[], summarized_at?, summary_model?,
            created_at, updated_at)
  article_embeddings          -- single active embedding space per instance (see §13)
       (article_id→articles CASCADE PK, embedding vector(<dim>), model, created_at)  + HNSW index

Subscriptions & Organization (PER-USER)
  user_feeds (id, user_id→users, feed_id→feeds, custom_name?, tags text[], settings jsonb,
              subscribed_at, updated_at, UNIQUE(user_id, feed_id))
  user_categories (id, user_id→users, name, parent_id?→user_categories, icon, color, order,
                   collapsed, sort_order, include_in_search, is_default, is_read_only, settings jsonb,
                   UNIQUE(user_id, name))
  user_feed_categories (id, user_feed_id→user_feeds, user_category_id→user_categories,
                        UNIQUE(user_feed_id, user_category_id))

Reading State & Learning (PER-USER)
  read_articles (id, user_id, article_id, read_at, UNIQUE(user_id, article_id))
  seen_articles (id, user_id, article_id, seen_at, UNIQUE(user_id, article_id))      -- NEW (P1)
  bookmarks (id, user_id, article_id, created_at, note?, UNIQUE(user_id, article_id)) -- NEW (favorites)
  article_feedback (id, user_id, article_id, feedback_type, feedback_value, time_spent?, estimated_time?,
                    created_at, updated_at, UNIQUE(user_id, article_id))
  user_patterns (id, user_id, keyword, weight, feedback_count, created_at, updated_at, UNIQUE(user_id, keyword))

Saved Searches (PER-USER)
  saved_searches (id, user_id, name, query, icon, threshold, category_id?, notify_on_match,
                  notify_threshold, daily_digest, recency_bias, priority_sources jsonb,
                  last_matched_at?, total_matches, archived, created_at, updated_at)
  saved_search_matches (id, saved_search_id→CASCADE, article_id→CASCADE, relevance_score,
                        matched_terms jsonb, match_reason?, notified, created_at,
                        UNIQUE(saved_search_id, article_id))

Preferences & Notifications (PER-USER)
  user_preferences (user_id UNIQUE, ... 30+ typed columns ...)   -- see PRD §5.10
  user_notifications (id, user_id, type, title, message, metadata jsonb, read, created_at, updated_at)

System / Ops
  admin_settings (id, key UNIQUE, value jsonb, description?, created_at, updated_at)
  cost_ledger (id, user_id?, provider, model, kind[embedding|summary|digest], input_tokens,
               output_tokens, cost_usd, occurred_at)                                -- NEW: durable costs
  agent_runs (id, agent_name, status[PENDING|RUNNING|SUCCESS|FAILED], started_at, completed_at?,
              duration_ms?, stats jsonb, logs jsonb, error_message?, trigger[SCHEDULER|MANUAL|SYSTEM|EVENT], metadata jsonb)
  agent_locks (agent_name PK, locked_at, locked_by, expires_at)                     -- lease-based singleton
  job_queue (id, kind, payload jsonb, run_at, attempts, max_attempts, status, locked_by?, locked_at?, created_at)
             -- apalis-managed (or SKIP LOCKED); transactional with content writes via outbox
  outbox_events (id, kind, payload jsonb, created_at, processed_at?)                -- e.g., ArticlesIngested
```

### 6.2 Indexing highlights
- `feeds(url)` unique; `feeds(health_status)`, `feeds(last_fetched)`; one primary-key `feed_icons` row per shared feed.
- `feed_catalog_entries(feed_id)` unique; published facet/order indexes on category, country, language, featured, and sort order.
- `articles(url)` unique; `articles(feed_id, published_at DESC)`, `articles(guid)`, `articles(content_hash)`, `articles(published_at DESC)`.
- `article_embeddings`: **HNSW** on the `vector` column (`vector_cosine_ops`); dimension fixed to the instance's active model.
- `user_feeds(user_id)`, `user_feeds(feed_id)`, GIN on `user_feeds(tags)`.
- `read_articles(user_id, read_at)`, `article_feedback(user_id, created_at)`.
- `user_patterns(user_id, weight)`.
- `saved_search_matches(saved_search_id, relevance_score)`, `(article_id)`.
- `job_queue(status, run_at)` (partial index on pending) for `SKIP LOCKED` polling.

### 6.3 Full-text search
For keyword search and the keyword component of saved searches, add a Postgres **`tsvector`** (generated column) on `articles(title, excerpt, content)` with a GIN index, so keyword matching is index-accelerated rather than `ILIKE`-scanning.

---

## 7. Multi-User Deduplication Design (Critical)

This section operationalizes PRD §6.

### 7.1 Storage: shared vs. personal (enforced by schema)
- **Shared, one row:** `feeds`, derived `feed_icons`, editorial `feed_catalog_entries`, `articles`, `article_embeddings`, article `summary/key_points/topics`, `feed_error_log`, `domain_extraction_state`.
- **Personal:** `user_feeds`, `user_categories`, `read_articles`, `seen_articles`, `bookmarks`, `article_feedback`, `user_patterns`, `saved_searches(+matches)`, `user_preferences`, `user_notifications`.
- **Derived/personal cache (not persisted authoritatively):** per-user relevance scores.

### 7.2 Write-path dedup (idempotent upserts)
- **Feed creation:** normalize URL → `INSERT ... ON CONFLICT (url) DO NOTHING RETURNING id` (or select existing). Subscription is a separate `user_feeds` upsert. A feed shared by N users has exactly one `feeds` row.
- **Catalog selection:** an admin can attach one editorial catalog entry to a shared feed; users may subscribe to many published catalog feeds in one transaction. The catalog never substitutes for arbitrary user URL creation.
- **Personal organization:** folders/tags and multi-folder feed links update only `user_categories`, `user_feeds`, and `user_feed_categories`; they never enqueue shared work or duplicate shared feeds/articles.
- **Article ingest:** dedup order = **GUID (feed-scoped) → URL (global unique) → content_hash (feed-scoped)**. Insert via `ON CONFLICT (url) DO UPDATE` only when `should_update` (content hash/title/published changed). One row per article regardless of subscriber count.

### 7.3 Compute-path dedup (claim + idempotency, O(1) per content version)
The heavy operations — **fetch, extract, embed, summarize** — are keyed by the **shared entity id** and executed at most once per content version:

- **Fetch/refresh** is scheduled **per feed**, not per subscription. The FeedRefresh agent builds the set of *due* feeds (union across all subscribers, deduped) and enqueues one `FetchFeed{feed_id}` job each. Conditional GET (`ETag`/`Last-Modified`) avoids re-downloading unchanged feeds.
- **Extraction/embedding/summarization** are enqueued **per article id**. Before doing work, the agent **claims** the article for that operation:
  - *Extraction:* only for newly-ingested articles (dedup happens first); `extraction_status` gates re-runs.
  - *Embedding:* skip if a row already exists in `article_embeddings`; use `INSERT ... ON CONFLICT (article_id) DO NOTHING` so concurrent workers don't double-compute.
  - *Summary:* first-writer-wins; skip if `summary/key_points/topics` already set. A short-lived Redis/advisory lock (`work:{op}:{article_id}`) prevents two workers computing the same summary simultaneously; the DB `ON CONFLICT` is the durable guard.
- **Result:** for a feed/article shared by N users, network + CPU + LLM work per refresh cycle is **O(1)**, not O(N). Only cheap per-user rows (notifications, saved-search matches, read state) scale with N.

### 7.4 Read-path efficiency
- The article list for a user is a join of the shared `articles` with the user's `user_feeds`/categories plus `LEFT JOIN read_articles/bookmarks/feedback` for personal flags. Shared columns are read once; personal flags are per-user.
- Relevance scores are computed per user but **cached** (`score:{user}:{article}`); the shared embedding/summary are **cached once** for all users (`embedding:{article}`, `summary:{article}`).

### 7.5 Delete/retention safety
- Unsubscribing deletes only `user_feeds` (+ dependent personal rows via cascade). Shared `feeds`/`articles` remain for other subscribers.
- Retention/cleanup operates on shared content globally, honoring the settings cascade and **preserving bookmarked** articles (fixes `preserveStarred`). Orphan feeds (no subscribers) may be GC'd by a maintenance agent per policy.

---

## 8. The AGENTS Framework (Agentic Background Workflows)

"AGENTS" are the autonomous background workflows that keep content fresh, enriched, and personalized. They are the heart of the dedup guarantee and are designed to be **uniform, observable, coordinated, and extensible**.

### 8.1 The `Agent` abstraction

```rust
/// A unit of autonomous background work. Implementations live in `crates/agents`.
#[async_trait]
pub trait Agent: Send + Sync + 'static {
    /// Stable, unique name (used for locks, run records, metrics).
    fn name(&self) -> &'static str;

    /// Cron schedule for time-driven agents; None for purely queue/event-driven ones.
    fn schedule(&self) -> Option<CronSchedule> { None }

    /// Should the framework hold a distributed singleton lock for a run? (default yes)
    fn singleton(&self) -> bool { true }

    /// Max in-flight jobs this agent processes concurrently.
    fn concurrency(&self) -> usize { 1 }

    /// Execute one invocation. `ctx` provides DB, cache, clock, providers, logger, cancellation.
    async fn run(&self, ctx: &AgentContext, job: Job) -> Result<AgentOutcome, AgentError>;
}
```

- **`AgentContext`** carries injected ports (repositories, `Cache`, `SafeFetchClient`, `Embedder`, `LlmClient`, `VectorStore`, `Clock`), a `JobLogger`, a cancellation token (graceful shutdown), and config.
- **`Job`** is a typed payload (see 8.3). Time-driven agents receive a synthetic "tick" job.
- **`AgentOutcome`** reports `stats` (counts) merged into the `agent_runs` record.

### 8.2 Runtime services
1. **Scheduler** (cron, via apalis/`tokio-cron-scheduler`): on each tick, enqueues the agent's job. Cron expressions come from config (e.g., `FEED_REFRESH_SCHEDULE`).
2. **Queue** (`apalis` over Postgres, or hand-rolled `SELECT ... FOR UPDATE SKIP LOCKED`): durable, at-least-once, with retries + exponential backoff and `max_attempts` → dead-letter.
3. **Singleton lock** (lease): before a singleton run, acquire `agent_locks(name)` via Postgres advisory lock or lease row with `expires_at` (default lease 10 min, matching reference `JOB_LOCK_TIMEOUT`); auto-recover expired leases. Prevents duplicate runs across instances.
4. **Run tracking:** every run writes an `agent_runs` row (`RUNNING`→`SUCCESS/FAILED`, duration, stats, captured logs, trigger). Powers the admin Jobs view.
5. **Event bus (outbox):** content writes emit `outbox_events` (e.g., `ArticlesIngested{article_ids}`) in the same transaction; a dispatcher turns them into queue jobs (and optionally a Redis pub/sub nudge for low latency). This is how "new articles" fan out to embedding/saved-search agents **without** per-user duplication.
6. **Idempotency & claims:** helpers for `claim_for(op, entity_id)` and idempotent upserts (§7.3).

### 8.3 Concrete agents (v1) and the pipeline

```
                          cron tick
                             │
                   ┌─────────▼──────────┐
                   │ FeedRefreshAgent   │  singleton, scheduled (default */5)
                   │ - compute due feeds │  (dedup feeds across all users)
                   │ - enqueue FetchFeed │
                   └─────────┬──────────┘
                             │ FetchFeed{feed_id}
                   ┌─────────▼──────────┐
                   │ FetchFeedAgent     │  concurrency=N (bounded)
                   │ - conditional GET   │  (SafeFetchClient, feed-rs)
                   │ - parse + dedup     │  (GUID/URL/hash)
                   │ - upsert articles   │  → outbox: ArticlesIngested
                   │ - record health     │  → per-user refresh notifications
                   └───┬─────────┬──────┘
      ArticlesIngested │         │ ArticlesIngested
             ┌─────────▼──┐  ┌───▼───────────────┐
             │ ExtractCon │  │ EmbedArticleAgent │  (once per article/space)
             │ tentAgent  │  │ - skip if present  │
             │ (rate-lim, │  │ - batch encode     │
             │  per new)  │  │ - store vector     │
             └─────┬──────┘  └───┬───────────────┘
                   │ (updated)     │ EmbeddingReady
                   │         ┌─────▼───────────────┐
                   │         │ SavedSearchMatch     │  (event-driven)
                   │         │ - pre-embed queries  │
                   │         │ - 0.6·sem+0.4·kw     │
                   │         │ - upsert matches     │
                   │         │ - notify ≥ threshold │
                   │         └──────────────────────┘
                   ▼
         SummarizeArticleAgent (on-demand / optional auto): first-writer-wins summary

  Scheduled maintenance agents (singleton):
    • PatternDecayAgent  (decay + prune per-user patterns)      -- was API-only/untracked in ref
    • CleanupAgent       (retention; preserves bookmarks; VACUUM ANALYZE when needed)
    • FeedHealthSweep    (optional; disable chronically failing feeds, batch notifications)
    • (v2) ClusterAgent  (group related articles into events)
    • (v2) DigestAgent   (LLM digest per cluster / daily saved-search digests)
```

Notes:
- **Ordering/reuse:** embedding depends on ingest; saved-search matching depends on embedding (uses cosine) + keyword. The event bus sequences these without per-user fan-out.
- **Per-user work is intentional & cheap:** refresh notifications and saved-search matches are inherently per-user, but they operate on already-shared, already-enriched articles.
- **Backpressure:** each agent has bounded concurrency; extraction is additionally throttled per domain (§12).

### 8.4 Adding a new agent (extensibility)
1. Implement `Agent` in `crates/agents`, define its `Job` payload.
2. Register it with the runtime (name, schedule/concurrency).
3. If event-driven, subscribe it to an `outbox_events` kind.
4. Runs, locks, metrics, logs, and the admin UI work automatically.

This uniformity is what lets the v2 backlog (clustering, digests) land without touching the framework.

---

## 9. Request Lifecycle & Web Layer

- **Middleware stack (tower):** request-id/tracing → timeout → body limit → compression → CORS (API) → session (tower-sessions) → auth extractor → CSRF (mutations) → rate-limit (auth/expensive) → router.
- **Handlers** are thin: extract + validate DTO (`garde`) → call a domain service → render Askama template (HTML) or JSON envelope (API). HTMX requests return **HTML partials** for `hx-swap`.
- **Server-rendered UI:** full pages for first load; HTMX partials for interactions (list pagination/infinite scroll, reading panel, feedback buttons, feed management forms, bulk actions, notifications). Alpine.js handles purely-local UI state (dropdowns, modals, resizable panel). Preferences (theme/typography) are applied **server-side** into rendered HTML/CSS variables so there's no flash and no client round-trip.
- **DTOs & envelopes:** API success `{ "data": ..., "message"?: ... }`, error `{ "error": ..., "details"?: ... }`; status codes per PRD §5 FR-API-2. A single `AppError → IntoResponse` mapping (see §19) ensures consistency and no internal leakage.
- **Progressive enhancement:** core flows (read, mark-read, add feed, feedback) work without JS via standard form posts; HTMX enhances them. Matches the reference's Server-Actions philosophy.

### 9.1 Open-Standard Client API (GReader compatibility)

RustSS ships a **clean native JSON API** *and* a **Google Reader API (GReader)-compatible** adapter so the large ecosystem of existing RSS clients (Reeder, FeedMe, FocusReader, NetNewsWire, Fluent Reader, …) works out of the box — the same approach FreshRSS and Miniflux take. (PRD FR-API-5.)

- **Design:** the GReader layer is a **thin adapter** mounted under `/api/greader/` (e.g. `/accounts/ClientLogin`, `/reader/api/0/*`). It translates GReader requests/responses to/from the same domain services the native API and UI use — no duplicate business logic. Its job is purely protocol/shape translation and identifier mapping.
- **Auth:** `POST /accounts/ClientLogin` (email + password → `Auth` token) and a write **`T` token** (`/reader/api/0/token`). Tokens are minted as **app-specific tokens** (stored hashed) so both local-account and OAuth users can generate a client credential without exposing their real password. Rate-limited like other auth endpoints.
- **Core endpoints (v1 target):**
  - `GET /reader/api/0/user-info`
  - `GET /reader/api/0/subscription/list`, `POST /reader/api/0/subscription/edit` (subscribe/unsubscribe/rename/label)
  - `GET /reader/api/0/tag/list` (folders/labels ↔ user categories; `state/com.google/starred` ↔ bookmarks)
  - `GET /reader/api/0/stream/contents/{streamId}`, `GET /reader/api/0/stream/items/ids`, `POST /reader/api/0/stream/items/contents`
  - `POST /reader/api/0/edit-tag` (read/unread, starred, etc.), `POST /reader/api/0/mark-all-as-read`
- **Identifier & state mapping:**
  - Stream IDs: `feed/{url}`, `user/-/state/com.google/reading-list|read|starred`, `user/-/label/{category}`.
  - Article "items" reference the **shared** `articles`; read state → `read_articles`, starred → `bookmarks`, labels → `user_categories`/`user_feed_categories`. All per-user, so multi-user isolation holds.
- **Non-goals:** 100% coverage of every historical GReader quirk. We target the subset that popular clients actually use (matching Miniflux/FreshRSS behavior), and document supported endpoints. Conformance is guarded by integration tests that replay real client request sequences.

---

## 10. Caching Strategy

Two-tier cache behind a single `Cache` port, with **shared vs. per-user key separation** (enables cross-user reuse — PRD §6 FR-DEDUP-6).

- **L1 — `moka`** (in-process, always on): microsecond hits for hot shared data (feed metadata, embeddings, summaries, parsed query ASTs). Bounded by entry count/size; TTL per namespace.
- **L2 — Redis (`fred`)** (shared across instances): survives single-instance restarts, enables cross-instance invalidation via pub/sub. Optional; if down, L1+DB serve.
- **Pattern:** `cache_get_or_set(key, ttl, || fetch())` (cache-aside), storing only non-null. Write-through where cheap.
- **Invalidation:** **tag/prefix based via `SCAN`** (never `KEYS`), plus targeted deletes. Redis pub/sub broadcasts invalidations so all instances drop L1 entries. Examples: on feedback → invalidate `score:{user}:*` and `patterns:{user}`; on article update → `*:{article_id}`; on feed refresh → `feed:{feed_id}*`.

**Key namespaces & default TTLs** (parity with reference, corrected):

| Key | Scope | TTL |
|---|---|---|
| `feed:{feed_id}` | shared | 1h |
| `feed:articles:{feed_id}:{page}` | shared | 5m |
| `embedding:{article_id}` | shared | 24h |
| `summary:{article_id}` / `keypoints` / `topics` | shared | 7d |
| `search:{hash(query,opts)}` | shared | 30m |
| `related:{article_id}:{opts}` | shared | 24h |
| `savedsearch:ast:{hash(query)}` | shared | 24h |
| `savedsearch:results:{id}` | per-user | 5m |
| `score:{user}:{article}` | per-user | 1h |
| `scores:{user}` (batch) | per-user | 1h |
| `patterns:{user}` | per-user | 15m |
| `prefs:{user}` | per-user | 1h |
| `subs:{user}` / `categories:{user}` | per-user | 15m / 30m |

Authoritative data (scores also persist? no — derived; summaries/embeddings persist in DB) — cache is an accelerator, DB is source of truth.

---

## 11. Core Algorithms & Constants

Defaults mirror the reference (validated during analysis); all are configurable.

### 11.1 Article deduplication
- `content_hash = SHA256(trim(content))` (hex).
- Match order: GUID(feed-scoped) → URL(global) → content_hash(feed-scoped).
- `should_update` if content_hash changed, or title changed, or `|published_new − published_old| > 60s`.

### 11.2 Personalization
- **Keyword extraction:** strip HTML → lowercase → tokenize on non-word → drop tokens len ≤ 2 and ~130 stop-words → term frequency normalized by token count; heuristic weight ×1.5 if raw count ∈ [2,10], ×0.5 if > 10. (TF with heuristics; not full IDF.)
- **Pattern update:** `Δweight = feedback_value × relevance × 0.1`, upsert into `user_patterns`, increment `feedback_count`.
- **Decay:** for patterns older than 30 days, `w ← w × 0.9^floor(days_since_update / 30)`.
- **Prune:** drop `|w| < 0.1`; keep top **100** by `|w|` per user.
- **Feedback values:** explicit 👍 `+1.0` / 👎 `−1.0`; implicit completion (≥ 0.9 of estimated time) `+0.5`, bounce (< user `bounce_threshold`, default 0.25) `−0.5`; explicit never overwritten by implicit.
- **Relevance score:** extract top-30 keywords of the article; `total = Σ pattern_weight(kw) × relevance(kw)`; `score = 1 / (1 + e^(−5 × total))`; neutral **0.5** if no patterns. Cached 1h with matched-keyword explanations.

### 11.3 Semantic search & related
- **Similarity** from pgvector cosine distance: `similarity = 1 − (embedding <=> query) / 2` ∈ [0,1].
- **Recency-weighted score** (when `recency_weight w > 0`): `final = (1−w)·similarity + w·exp(−age_seconds / (decay_days·86400))`.
- Defaults: `limit = 10`, service `min_score = 0.5` (UI may pass 0.6/0.7), `recency_weight = 0.3`, `recency_decay_days = 30`.
- **Related:** rank by `article.embedding <=> source.embedding` (same space).

### 11.4 Saved searches
- **DSL → AST:** OR `,`; required AND `+`; NOT `-`; phrase `"..."`; group `(...)`; adjacency = implicit AND. Precedence: phrase > exclusion > requirement > alternatives.
- **Boolean filter:** term = `\bword\b`, phrase = substring, over `title+excerpt+content` (lowercased). Fail ⇒ no match.
- **Keyword score:** per term `tf = 1 + ln(occurrences)`, ×`2.0` if in title; `score = min(Σ / (n_terms·3), 1.0)`.
- **Combined:** `0.6·semantic + 0.4·keyword`, capped 1.0. Match iff `≥ threshold` (default 0.6). Notify iff `≥ notify_threshold` (default 0.85).
- **Throughput target:** ≥ 1000 articles/min; batch 100 articles × concurrency 5; pre-embed each query once per batch.

### 11.5 Settings cascade
Effective value = first present of **feed → category → user prefs → system defaults → env** (feed wins). Applies to `refresh_interval`, `max_articles_per_feed`, `max_article_age`, and the `summarization` block. Constraints (admin): refresh 15–1440 min, max_articles 50–5000, max_age 1–365 days.

### 11.6 Cleanup/retention
- Delete articles older than `max_article_age` **and** trim each feed to newest `max_articles_per_feed`; **preserve bookmarked**. Age basis is explicit/configurable (`published_at` preferred; `createdAt` available for parity). `VACUUM ANALYZE` when deletions exceed a threshold.

---

## 12. Content Extraction & Safe Fetch

- **`SafeFetchClient`** wraps `reqwest` and is the **only** way any code makes outbound HTTP (feeds, article pages, robots.txt, media proxy). It enforces: scheme allow-list (http/https), DNS resolution + **private/link-local/loopback IP blocking** (IPv4 & IPv6, incl. mapped), redirect re-validation (guards DNS-rebinding/redirect-to-internal), size caps, and timeouts. This closes the reference's SSRF gaps (PRD §9.6).
- **Strategies** via an `Extractor` trait + registry with priority + fallback:
  1. **Readability** (`article_scraper`): fetch HTML → parse → extract main content, images, metadata. Default.
  2. **Chromium** (`chromiumoxide`, pooled): future option for public JS-rendered pages; `networkidle` and optional wait-selector, then Readability on rendered DOM. No cookie injection or authenticated/paywall extraction; falls back to Readability if unavailable.
- **Per-domain rate limiting:** shared `domain_extraction_state`; base delay default **3000 ms**, overridable per feed; honor robots.txt `Crawl-delay` (cached 24h/1h-on-error); adaptive `429` backoff **10s→30s→60s→120s** honoring `Retry-After`. Extraction runs **only for new articles** and **once per shared article** (§7.3).
- **Sanitization:** all extracted/stored HTML is cleaned with **`ammonia`** (allow-list of tags/attrs, safe links, no scripts) before persistence and again defensively at render.
- **Publisher access:** public anonymous page requests only. Publisher access restrictions fall back to RSS content; RustSS does not store feed cookies or bypass publisher controls. Each shared feed can explicitly respect or ignore robots.txt for public-page extraction.

---

## 13. Embeddings, Semantic Search & Vector Storage

### 13.1 One embedding space per instance (the correctness fix)
**Decision (confirmed):** an instance uses exactly **one** embedding model. The reference stores 384-dim local vectors zero-padded into a single `vector(1536)` column shared with 1536-dim OpenAI vectors — silently corrupting similarity when providers mix. We eliminate the whole failure mode by pinning a single space:

- An **embedding space** = `(model, dimension, distance)`, e.g. `local_bge_384` (384, cosine). The active space is fixed at deploy via `EMBEDDING_MODEL`.
- Embeddings live in a single table `article_embeddings(article_id PK → articles, embedding vector(<dim>), model, created_at)` whose column dimension matches the active model, with **one HNSW index** (`vector_cosine_ops`). (A pgvector column has a fixed dimension, so the dim is a deploy-time constant baked into the migration for that space.)
- The stored `model` is **guarded**: writes assert the vector's model matches the instance's active model; a mismatch is a hard error, never a silent pad/truncate.
- **Changing the model** is an explicit admin operation: create the new-dim table/index, run `EmbedArticleAgent` to re-embed all articles, then swap the active space and drop the old table. Documented in `docs/MIGRATION.md`.
- A **`VectorStore` port** abstracts this so the multi-space design remains *possible* later (per-model tables) without complicating the v1 common case.

### 13.2 Generation
- **Local (default):** `fastembed` (ONNX/`ort`), BGE-small-en-v1.5, mean-pooled + L2-normalized, 384-dim, CPU. **Model weights are downloaded on first run** (confirmed) into a persistent cache dir (`MODEL_CACHE_DIR`, mounted as a volume) — the image stays small; first run needs network. Load is lazy + cached in-process; a startup warm-up is optional.
- **OpenAI-compatible:** `async-openai`, batch up to provider limit; typed cost via `cost_ledger`.
- **Text embedded:** `title + "\n\n" + excerpt + "\n\n" + content[..2000]`. Batched (configurable size, default 10; streaming batch 25). Idempotent insert (skip if present) → O(1) across users.

### 13.3 Query path
- Embed the query with the instance's model → pgvector KNN (`ORDER BY embedding <=> $1 LIMIT k`) with HNSW → normalize similarity → optional recency blend → filter by `min_score` → optional `feed_id`/date filters. Keyword/hybrid uses the `tsvector` index.

---

## 14. LLM Integration & Secrets

- **Provider cascade** (per operation: summary/embedding/digest): **user config → admin/system config → environment**. Providers: OpenAI-compatible (incl. Azure, Ollama `/v1`) and Ollama native.
- **Summarization** (`SummarizeArticleAgent` / on-demand): prompt returns JSON `{summary, key_points[], topics[], sentiment?}`; robust JSON extraction with text fallback; content truncated to a safe max; results stored **once** on the shared article (first-writer-wins).
- **Durable cost tracking (fix):** every billable call appends to **`cost_ledger`** (user, provider, model, kind, tokens, `cost_usd`, timestamp). Admin/user views aggregate by day/month/provider. Replaces the reference's in-process counters (PRD §9.3). Pricing table includes `gpt-4o-mini` (the common default) to avoid the reference's mis-pricing bug.
- **Secrets (unified AEAD — fix):** stored LLM/API credentials use **XChaCha20-Poly1305**; the 32-byte key is derived from `ENCRYPTION_SECRET` via **HKDF-SHA256** (domain-separated per secret type). One format: `base64(nonce ‖ ciphertext ‖ tag)` with associated data = secret type. Secrets are never logged; masking helper for display. Feed-cookie extraction is intentionally unsupported. (Replaces the reference's two incompatible schemes; §21 covers migration.)

---

## 15. Authentication & Authorization

- **OAuth2/OIDC** (implemented directly over `reqwest`; FND-117): Google, GitHub, and a configurable generic OIDC provider; authorization-code flow with PKCE (S256) + `state` CSRF (both stashed in short-lived HTTP-only cookies between redirect and callback); profile fetched from userinfo; account linked in `oauth_accounts` (find-by-subject → match-by-email → create).
- **Local accounts:** email + password hashed with **Argon2id**; optional email verification; registration mode (open/invite/closed) via admin/env; **first registered user becomes ADMIN** if none exists (self-host bootstrap).
- **Sessions:** a small, dependency-light DB-backed scheme (FND-116) — the browser holds an opaque random token in a secure, HTTP-only, SameSite=Lax cookie; only its SHA-256 hash is stored in `sessions`, so a DB read never exposes live tokens. CSRF is a double-submit cookie token for cookie-auth mutations.
- **Authorization:** role checks (`ADMIN|USER|GUEST`) via an extractor/middleware; every data query is **scoped by `user_id`** for per-user tables; shared-content reads require an active subscription where appropriate. Admin routes gated server-side.
- **Rate limiting** on login/register/password endpoints (`tower_governor`).

---

## 16. Configuration

- **Source:** environment variables (12-factor) layered with an optional file via **`figment`**; `.env` supported in dev (`dotenvy`). Parsed into a **typed, validated `AppConfig`** at startup; the process refuses to boot on invalid/missing required config (parity with the reference's `@t3-oss/env` approach).
- **Key groups** (superset of reference): database URL; Redis URL/password + `CACHE_ENABLED`; auth (`SESSION_SECRET`, OAuth client IDs/secrets, provider URLs, `REGISTRATION_MODE`); embeddings (`EMBEDDING_PROVIDER`, `EMBEDDING_MODEL` — the single active model, `MODEL_CACHE_DIR` for downloaded weights, batch sizes, `EMBEDDING_AUTO_GENERATE`); LLM (`LLM_PROVIDER`, models, `OPENAI_API_KEY`, `OPENAI_BASE_URL`, `OLLAMA_BASE_URL`); extraction (`EXTRACTION_*`, `CHROMIUM_ENABLED`/endpoint, `EXTRACTION_DELAY_MS`, `RESPECT_ROBOTS_TXT`); jobs (`ENABLE_CRON_JOBS`, `FEED_REFRESH_SCHEDULE`, `CLEANUP_SCHEDULE`, `FEED_REFRESH_CONCURRENCY`, `SAVED_SEARCH_CONCURRENCY`, `JOB_LOCK_TIMEOUT`, `RUN_EMBEDDED_WORKER`); crypto (`ENCRYPTION_SECRET`); server (`BIND_ADDR`, `BASE_URL`, `LOG_LEVEL`).

---

## 17. Observability & Operations

- **Logging/tracing:** `tracing` with JSON or pretty output; `tower-http::TraceLayer` for HTTP spans; correlation IDs; agent runs carry a span + captured log ring buffer persisted to `agent_runs.logs`.
- **Metrics:** `metrics` + Prometheus exporter at `/metrics`: HTTP latency/count/status, refresh stats (feeds fetched/304/new/updated), queue depth & job durations, cache hit-rate (L1/L2), embedding/LLM tokens & cost, extraction rate-limit events.
- **Health:** `/api/health` (liveness) and `/api/health/ready` (readiness: DB required; Redis/LLM/Chromium reported as degraded, not failed).
- **Admin ops:** job history/trigger, cache clear, VACUUM/ANALYZE, embedding backfill/delete, cleanup, storage/cost views, user/role management.
- **Backups:** documented `pg_dump`/restore; Redis is disposable.

---

## 18. Deployment Topology

- **Default (single-node, self-hosted):** one container running server **with embedded worker** (`RUN_EMBEDDED_WORKER=true`) + Postgres/pgvector + Redis + headless Chromium, via **Docker Compose**. Lowest ops overhead.
- **Scaled:** N stateless `rustss-server` replicas (no embedded worker) behind a load balancer + M `rustss-worker` replicas; all share Postgres + Redis. Agents coordinate via advisory locks + the Postgres queue, so scheduled jobs run once and queue work is load-balanced.
- **Images:** multi-stage build → small runtime image (distroless/Debian-slim), non-root, multi-arch (amd64/arm64). Chromium in a separate layer/service to keep the app image `< 150 MB`.
- **Migrations:** applied by an entrypoint/CLI step (`rustss-cli migrate`) before serving; safe/idempotent; opt-in auto-run.
- **Assets:** Tailwind compiled at build; CSS/JS embedded via `rust-embed` for a single self-contained binary.

---

## 19. Error Handling Strategy

- **Domain crates:** `thiserror` enums per module (e.g., `FeedError`, `ExtractError`, `EmbedError`, `RepoError`) — precise, matchable, no stringly-typed errors.
- **Application/bins:** `anyhow` at the outermost boundary and in agents where context-chaining aids debugging.
- **Web layer:** a single `AppError` type (implements `IntoResponse`) maps domain errors → HTTP status + safe JSON/HTML; validation errors → 400 with field `details`; auth → 401/403; not-found → 404; conflict/dup → 409; rate-limit → 429; everything else → 500 with a log-only internal message (never leak internals).
- **Agents:** errors are recorded to `agent_runs`, increment failure metrics, and drive retry/backoff or dead-letter; feed fetch/extract errors additionally update feed health.
- **Concurrency:** prefer idempotent operations + `ON CONFLICT`; treat unique-violation as benign "already done".

---

## 20. Testing Strategy

- **Unit:** pure algorithms (content hash & dedup ordering, TF keyword extraction, pattern update/decay/prune, sigmoid scoring, similarity/recency math, saved-search DSL parser & scorer, settings cascade). These have exact expected values from §11 for regression against the reference.
- **Integration:** repositories & agents against **ephemeral Postgres+pgvector** (testcontainers/`sqlx::test`); verify dedup guarantees (N-subscriber feed ⇒ 1 fetch/embed/summary), conditional GET (304 handling), and cascade resolution.
- **HTTP:** router/handler tests (auth, CSRF, envelopes, status codes) with a test app; **`wiremock`** for feed/LLM/HTTP fakes.
- **Browser E2E:** Playwright with Chromium starts the actual Rust server on an isolated port and exercises catalog selection, shortcut capture/conflict handling, preference persistence, and authorization boundaries.
- **Property tests** (`proptest`) for the query parser and dedup invariants.
- **Load/bench** (`criterion` for hot functions; a small harness for search/list p95 and refresh throughput) to guard PRD §7 targets.
- **CI gates:** `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, locked Playwright/Chromium browser E2E, migration check, and a smoke `docker compose --env-file .env -f docker/compose.yaml up` health probe.

---

## 21. Migration from the Reference Implementation

The reference uses Postgres/pgvector already, so migration is primarily **schema reshaping + secret re-encryption**, not a datastore change.

1. **Schema:** create the Rust schema; map reference tables 1:1 where identical (users, feeds, articles, user_feeds, categories, read/feedback/patterns, saved searches, preferences, notifications, admin_settings, cron/job tables → `agent_runs`/`agent_locks`).
2. **Embeddings:** migrate the reference's single `articles.embedding` column into `article_embeddings` **only if** the source rows match the instance's chosen active model/dimension; otherwise drop them and **re-embed** with the agent (correctness > preserving padded/mismatched vectors). Since RustSS pins one model per instance, this is a one-shot backfill.
3. **Secrets:** decrypt LLM keys/cookies using the reference's two schemes, re-encrypt under the unified AEAD (a one-shot `rustss-cli migrate-secrets`). If the old `ENCRYPTION_SECRET` is unavailable, affected users re-enter keys.
4. **IDs:** reference uses cuid/nanoid strings; keep them as-is for existing rows (columns are `text`/`uuid`-compatible) or remap with a translation table; new rows use UUIDv7.
5. **Bookmarks/seen:** new tables start empty; retention immediately honors bookmarks going forward.
6. **Cutover:** run both against the same DB is **not** supported; do a maintenance-window migration with `pg_dump` backup first.

A dedicated `docs/MIGRATION.md` will detail scripts once the schema is finalized.

---

## 22. Key Design Decisions (ADR summary)

| # | Decision | Rationale | Trade-off |
|---|---|---|---|
| 1 | **Axum + Askama + HTMX** (no SPA) | Low memory, fast, all-Rust, progressive enhancement; fits self-hosting | Some rich interactions need more server round-trips vs. an SPA |
| 2 | **SQLx (compile-checked SQL)** over an ORM | pgvector raw-SQL fit, low overhead, query correctness at compile time | Less "magic"; more explicit SQL |
| 3 | **Postgres-backed job queue** (Redis optional) | Durable, transactional with content writes (outbox), no extra infra dependency for correctness | Redis still used for cache/nudges; queue throughput bounded by Postgres |
| 4 | **One embedding space per instance** (single fixed-dim table + model guard) | Eliminates mixed-dim corruption; simplest correct design; one HNSW index | Model change requires an explicit re-embed/migration |
| 5 | **Unified XChaCha20-Poly1305 secrets** | One secure scheme; fixes dual-format debt | One-time secret migration required |
| 6 | **Server+worker one codebase, deploy 1 or 2 ways** | Simple by default, scalable when needed | Must keep worker/server wiring clean (DI at bin level) |
| 7 | **Centralized `SafeFetchClient`** | Uniform SSRF protection on every egress | All fetch paths must route through it (enforced by layering) |
| 8 | **UUIDv7 PKs** | Time-sortable, index-friendly, distributed-safe | Differs from reference's cuid (handled in migration) |
| 9 | **GReader-compatible API adapter** (native API + GReader layer) | Interop with the existing RSS client ecosystem; no duplicate business logic | Must track a de-facto (un-versioned) protocol; scoped to the popular subset |
| 10 | **Download embedding model on first run** (volume cache) | Small image; offline after first fetch | First run needs network; cold cache adds one-time download |

---

*See `docs/PRD.md` for requirements and success metrics, and `README.md` for the project guide and setup.*
