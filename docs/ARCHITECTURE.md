# Rusty Feed — Architecture Document

**Status:** Living architecture — implemented incrementally
**Version:** 0.1.0
**Last updated:** 2026-09-01
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

This document defines the technical architecture for Rusty Feed — a self-hosted, multi-user, AI-augmented RSS reader. It proposes a **complete tech stack**, the **data model**, the **background-agent framework**, and the strategies (caching, dedup, security, deployment) needed to satisfy the PRD, with special emphasis on the **multi-user content-deduplication** guarantee and **self-hosted performance/footprint**.

Behavioral parity with the reference is the baseline; §21 and Rusty Feed explicitly corrects these behaviors discovered during analysis of `neureed`.

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
| Interactivity | **Dependency-free local JavaScript + standard forms** | HTMX/Alpine | Server-driven UI with no SPA or runtime CDN; enhancements stay small and core flows retain normal-link/form fallbacks |
| Styling | **Curated CSS design system** | Tailwind | Embedded, offline, no frontend build/runtime dependency; FND-118 records the deliberate deviation |
| Static assets | **`rust-embed`** (embed CSS/JS/templates) | tower-http `ServeDir` | Single-binary distribution; assets baked in |
| CSRF | **custom double-submit token validation** | `axum-csrf` | Small auditable cookie-auth mutation protection integrated with the custom sessions |
| Rate limiting | **bounded in-process per-key limiter** (`crates/web/src/ratelimit.rs`) | — | Focused, dependency-free throttling for auth, API, feed-add, search, summarize, OPML, media, Fever, and GReader (configurable; `0` == unlimited) |

### 3.2 Data & Cache

| Concern | Choice | Alternative | Why |
|---|---|---|---|
| Database | **PostgreSQL 17 + pgvector** | — | System of record; vector search; the reference's data model is Postgres-native |
| DB access | **SQLx** (async; runtime queries + `FromRow`) | SeaORM, Diesel | Raw-SQL friendliness for pgvector, no heavy ORM, low overhead. We use runtime `query`/`query_as` (not `query!`) so builds are hermetic; SQL is validated by integration tests against real Postgres (FND-107) |
| Vector types | **pgvector extension via SQL text casts** | typed vector binding | Native `vector` columns and cosine operators without another Rust adapter |
| Migrations | **`sqlx::migrate!`** (embedded) | refinery | Versioned SQL migrations applied safely on startup |
| L1 cache | **`moka`** (in-process, async, TTL/LRU) | — | Always-on, zero-dependency hot cache; keeps Redis load down |
| L2 cache | **optional `redis` client** (pooled + Pub/Sub) | — | Namespace-isolated cache across instances with coherent L1 invalidation; single-node deployments can use L1 only |
| IDs | **UUIDv7** (`uuid`) | ULID | Time-sortable PKs, index-friendly, globally unique |
| Time | **`time`** | chrono | Lightweight, `timestamptz` support |
| Hashing | **`sha2`** | — | SHA-256 content hashes, token lookup hashes, and stable dedup keys |

### 3.3 Background Work & Scheduling

| Concern | Choice | Alternative | Why |
|---|---|---|---|
| Job framework | **custom PostgreSQL `SKIP LOCKED` queue and Tokio workers** | apalis | Durable lane isolation, retries/backoff, heartbeats, UTC cron schedules, transactional outbox, and inspectable state without an extra broker |
| Singleton locks | **Postgres lease table** + transactional guards | Redis `SET NX PX` | Exactly-one-runner semantics across instances; DB is authoritative |
| Event fan-out | **Postgres job queue** | future transactional outbox | Durable shared work without a Redis dependency |

> Rationale: the **queue remains in Postgres** so job state is durable and inspectable. Writers enqueue idempotently, and transaction-coupled events use the durable outbox dispatcher. Redis is an optional L2 cache, so its absence does not break a single node.

### 3.4 Feeds, Extraction, AI

| Concern | Choice | Alternative | Why |
|---|---|---|---|
| HTTP client | **`reqwest`** (rustls, gzip/br, timeouts) | hyper directly | Ergonomic outbound TLS via rustls, compression; WebAuthn attestation separately uses the platform OpenSSL verifier (FND-177) |
| Feed parsing | **`feed-rs`** | custom | RSS 1.0/2.0, Atom, JSON Feed in one crate |
| HTML parsing | **`scraper`/`html5ever`** | lol_html | Robust DOM for extraction & sanitization |
| Readability extraction | **`dom_smoothie`** (Mozilla Readability-compatible) + `scraper` fallback | `article_scraper`, `readability` | Strong boilerplate removal and semantic article recovery, bounded locally; configurable selector override and density fallback |
| Headless extraction | **`chromiumoxide`** (remote CDP, async) | fantoccini | Bounded renderer receives only SafeFetch-obtained HTML under a no-network CSP and degrades to local extraction |
| HTML sanitization | **`ammonia`** | — | XSS-safe cleaning of stored/rendered feed HTML |
| Local embeddings | **`fastembed` BGE-small-en-v1.5; explicit deterministic hash fallback** | candle | Production semantic inference is local; cache seeding and guarded model migration support offline hosts (FND-121) |
| ONNX runtime | **`ort` through fastembed** | — | Lazy bounded inference with a persistent model cache |
| LLM / OpenAI | **focused `reqwest` OpenAI-compatible clients** | async-openai | Covers OpenAI-compatible and Ollama `/v1` endpoints with a small tested surface |
| Ollama | **same focused `reqwest` client via `/v1`** | `ollama-rs` | Reuse the tested OpenAI-compatible surface; native API only if needed |
| Robots.txt | small parser (crawl-delay/allow) | `texting_robots` | Honor crawl-delay; cache per domain |

### 3.5 Auth, Validation, Crypto

| Concern | Choice | Alternative | Why |
|---|---|---|---|
| OAuth2/OIDC | **direct `reqwest` authorization-code flow with PKCE/state** | oauth2/openidconnect crates | Google/GitHub/Generic support with explicit rustls behavior and wiremock coverage (FND-117) |
| Sessions | **custom opaque-token PostgreSQL sessions** | tower-sessions | Only a SHA-256 token hash is stored; explicit expiry and cookie controls avoid library churn (FND-116) |
| Password hashing | **`argon2`** (Argon2id) | — | Local accounts, memory-hard hashing |
| Input validation | **bounded typed `serde` DTOs plus focused validators** | garde/validator | Validation stays near each route/config boundary without a broad framework |
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
                                  │ Browser (HTML + progressive JS) • API consumers │
                                  └───────────────┬───────────────────┬─────────────┘
                                                  │ HTML over HTTP     │ JSON over HTTP
                                                  ▼                    ▼
        ┌──────────────────────────────────────────────────────────────────────────────────┐
        │                             rusty-feed-server (Axum)                                  │
        │                                                                                    │
        │  tower-http (trace, compression, timeout, CORS) → auth/session → CSRF → routers    │
        │                                                                                    │
        │   ┌────────────┐   ┌────────────┐   ┌──────────────┐   ┌───────────────────────┐  │
        │   │ HTML pages │   │  JSON API  │   │  Auth (OAuth/ │   │  Admin dashboard      │  │
        │   │ (Askama)   │   │  handlers  │   │  local)       │   │  (stats/jobs/config)  │  │
        │   └─────┬──────┘   └─────┬──────┘   └──────┬───────┘   └──────────┬────────────┘  │
        │         └─────────────────┴─── application services (domain) ─────┘               │
        │                                     │                                              │
        └─────────────────────────────────────┼──────────────────────────────────────────────┘
                     │                          │                          │
                     ▼                          ▼                          ▼
             ┌───────────────┐         ┌────────────────┐        ┌──────────────────┐
             │  Postgres 17  │         │     Redis      │        │  Object/Model     │
             │  + pgvector   │◄──────► │ (optional L2   │        │  store: local     │
             │ (system of    │  cache  │  cache)        │        │  model/assets     │
             │  record +     │         │                │        │  storage          │
             │  job queue)   │         └────────────────┘        └──────────────────┘
             └──────┬────────┘
                    │ SKIP LOCKED
                    ▼
        ┌──────────────────────────────────────────────────────────────────────────────────┐
        │                     rusty-feed-worker (AGENTS runtime)                                │
        │ Interval scheduler → enqueue │ Bounded lane consumers │ Singleton leases/guards    │
        │                                                                                    │
        │  FeedRefresh → FetchFeed → ExtractContent → EmbedArticle → SavedSearchMatch        │
        │  Summarize (on-demand) • PatternDecay • Cleanup • Health • ClusterStories • Digest   │
        │                                                                                    │
        │ Outbound via SafeFetchClient • providers • bounded isolated Chromium fallback    │
        └──────────────────────────────────────────────────────────────────────────────────┘
```

The **server** and **worker** are the same codebase and can run **in one process** (default, self-hosted) or as **separate deployments** (horizontal scale). Both share the domain libraries; the worker hosts the AGENTS runtime, the server can optionally embed it (`RUN_EMBEDDED_WORKER=true`).

---

## 5. Workspace & Module Structure

A Cargo **workspace** with small, focused crates. Domain crates expose traits; adapters implement them. This keeps compile times reasonable, enables mocking, and enforces layering.

```
rusty-feed/
├─ Cargo.toml                 # workspace
├─ crates/
│  ├─ core/                   # config, errors, crypto, and shared pure algorithms
│  ├─ db/                     # SQLx repository modules + migrations + pgvector
│  ├─ cache/                  # moka L1 + optional Redis L2
│  ├─ feeds/                  # feed fetch (SafeFetchClient) + parse (feed-rs) + dedup
│  ├─ extract/               # Readability (dom_smoothie + scraper fallback) + future Chromium
│  │                          #   + per-domain rate limiter + robots.txt + sanitizer
│  ├─ embed/                  # deterministic local fallback + OpenAI-compatible embedder
│  ├─ llm/                    # OpenAI/Ollama chat clients + summarization + cost meter
│  ├─ search/                 # semantic search, saved-search DSL/AST/matcher, scoring,
│  │                          #   pattern detection (TF), personalization
│  ├─ agents/                 # AGENTS framework + concrete agents (uses all above)
│  └─ web/                    # Axum routes/handlers, Askama templates, embedded static
│                             #   assets, auth, progressive interactions, error mapping
├─ bins/
│  ├─ server/                 # main HTTP server (optionally embeds worker)
│  ├─ worker/                 # standalone AGENTS runtime
│  └─ cli/                    # migrate; other maintenance commands are tracked stubs
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
  sessions (id, user_id→users, token_hash UNIQUE, created_at, last_used_at, expires_at)
  local_credentials (optional split of password data)  -- or inline on users
  api_tokens (id, user_id→users, name, token_hash UNIQUE, scopes, last_used_at?, created_at, revoked_at?)  -- app-specific tokens (GReader/native API)

Feeds (SHARED)
  feeds (id, url UNIQUE(normalized), name, site_url, description, image_url,
         etag?, last_modified?, last_fetched?, last_successful_fetch?,
         fetch_interval_default, health_status[healthy|warning|error|disabled],
         consecutive_failures, failure_started_at?, error_count, notify_on_error,
         http_status?, redirect_url?, extraction_settings jsonb,  -- shared selector/safety mechanics
         respect_robots_txt, extraction_delay_ms?, created_at, updated_at)
  feed_error_log (id, feed_id→feeds CASCADE, ts, error_type, error_message, http_status?, details jsonb, resolved)
  feed_icons (feed_id→feeds CASCADE PK, source_url, canonical raster MIME + bounded BYTEA, attempt/fetch/error timestamps)
  feed_catalog_entries (id, feed_id→feeds UNIQUE, title, description?, category, country, language,
                        featured, sort_order, published, created_at, updated_at)
  catalog_content_probes (feed_id→feed_catalog_entries PK, candidate_article_ids[0..3], sampled_article_count,
                          aggregate anonymous-public-page outcomes, RSS/public-body character averages, checked_at)
  domain_extraction_state (domain UNIQUE, last_extracted_at, failure_count, rate_limit_count, crawl_delay_ms?, last_error?)

Articles (SHARED)
  articles (id, feed_id→feeds CASCADE, url UNIQUE, comments_url?, guid?, title, author?, content, excerpt?,
            content_hash, feed_content, feed_excerpt, article_page_content?, article_page_excerpt?,
            article_page_fetched_at?, article_page_expires_at?, image_url?, published_at?,
            extraction_status[NONE|SUCCESS|FAILED], extraction_error?,
            summary?, key_points jsonb?, topics text[], summarized_at?, summary_model?,
            created_at, updated_at)
  article_enclosures (article_id→articles CASCADE, position, url, media_kind[audio|video|image|file],
                      mime_type?, title?, size_bytes?, duration_seconds?, width?, height?,
                      PRIMARY KEY(article_id, url), UNIQUE(article_id, position))
  article_embeddings          -- single active embedding space per instance (see §13)
       (article_id→articles CASCADE PK, embedding vector(<dim>), model, created_at)  + HNSW index

Subscriptions & Organization (PER-USER)
  user_feeds (id, user_id→users, feed_id→feeds, custom_name?, tags text[], settings jsonb,
              -- settings owns personal content_mode (rss|article_page|live) + finite/infinite public-page lifetime
              subscribed_at, updated_at, UNIQUE(user_id, feed_id))
  user_categories (id, user_id→users, name, parent_id?→user_categories, icon, color, order,
                   collapsed, sort_order, include_in_search, is_default, is_read_only, settings jsonb,
                   UNIQUE NULLS NOT DISTINCT(user_id, parent_id, name))
  user_feed_categories (id, user_feed_id→user_feeds, user_category_id→user_categories,
                        UNIQUE(user_feed_id, user_category_id)) -- application presents one current folder/feed

Reading State & Learning (PER-USER)
  read_articles (id, user_id, article_id, read_at, UNIQUE(user_id, article_id))
  seen_articles (id, user_id, article_id, seen_at, UNIQUE(user_id, article_id))      -- NEW (P1)
  bookmarks (id, user_id, article_id, created_at, note?, UNIQUE(user_id, article_id)) -- NEW (favorites)
  article_feedback (id, user_id, article_id, feedback_type, feedback_value, time_spent?, estimated_time?,
                    created_at, updated_at, UNIQUE(user_id, article_id))
  user_patterns (id, user_id, keyword, weight, feedback_count, created_at, updated_at, UNIQUE(user_id, keyword))
  public_article_shares (id, user_id, article_id→articles, token_hash UNIQUE, token_ciphertext,
                         slug, content_source, visibility/image policy, expires_at?, revoked_at?,
                         UNIQUE(user_id, article_id))
  public_article_share_events (id, share_id?, user_id, article_id, action, token-free policy snapshot,
                               created_at) -- administrator-visible audit without bearer material

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
  instance_summary_policy (singleton PK, enabled, min_content_chars, updated_at)
  cost_ledger (id, user_id?, provider, model, kind[embedding|summary|digest], input_tokens,
               output_tokens, cost_usd, occurred_at)                                -- NEW: durable costs
  agent_runs (id, agent_name, status[PENDING|RUNNING|SUCCESS|FAILED], started_at, completed_at?,
              duration_ms?, stats jsonb, logs jsonb, error_message?, trigger[SCHEDULER|MANUAL|SYSTEM|EVENT], metadata jsonb)
  agent_locks (agent_name PK, locked_at, locked_by, expires_at)                     -- lease-based singleton
  job_queue (id, queue_name, kind, payload jsonb, run_at, attempts, max_attempts, status, locked_by?, locked_at?, last_error?, created_at)
             -- custom SKIP LOCKED queue; current writers insert jobs idempotently
  job_worker_heartbeats (worker_id PK, concurrency, interactive_concurrency, scheduler_enabled, started_at, last_heartbeat_at)
             -- live process/capacity signal; stale rows are ignored and periodically reclaimed
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
- `job_queue(queue_name, run_at)` (partial index on pending) for lane-isolated `SKIP LOCKED` polling. `interactive` has reserved low-latency consumers; `background` handles scheduled and bulk work.

### 6.3 Full-text search
Interactive keyword and hybrid search use two stored generated vectors on the shared `articles` row: language-neutral `search_vector_simple` and English-stemmed `search_vector`. Both weight title as `A`, excerpt as `B`, and content as `D`, and each has a GIN index. Migration `0026_indexed_full_text_search.sql` rebuilds existing rows automatically through PostgreSQL's generated-column semantics. The application selects only between these two migration-owned columns, so a user can never inject a `regconfig` or SQL identifier.

Queries use `websearch_to_tsquery`, providing forgiving quoted phrases, uppercase `OR`, implicit `AND`, and `-excluded` terms. Keyword rank is bounded with `ts_rank_cd(..., 32)`. Hybrid search unions bounded keyword candidates with model/revision-guarded HNSW candidates before applying the configured semantic/keyword blend; an exact keyword match therefore remains visible while its shared embedding is missing or stale. Every candidate query joins through `user_feeds(user_id, feed_id)` before ranking.

---

## 7. Multi-User Deduplication Design (Critical)

This section operationalizes PRD §6.

### 7.1 Storage: shared vs. personal (enforced by schema)
- **Shared, one row:** `feeds`, derived `feed_icons`, editorial `feed_catalog_entries`, `catalog_content_probes`, `articles`, `article_embeddings`, article `summary/key_points/topics`, `feed_error_log`, `domain_extraction_state`.
- **Personal:** `user_feeds`, `user_categories`, `read_articles`, `seen_articles`, `bookmarks`, `article_feedback`, `user_patterns`, `saved_searches(+matches)`, `user_preferences`, `user_notifications`.
- **Derived/personal cache (not persisted authoritatively):** per-user relevance scores.
- **Public-share authorization is personal; content remains shared:** `public_article_shares` stores only the owner's revocable policy and an encrypted bearer credential plus lookup hash. It references the one `articles` row and never copies a body.

### 7.2 Write-path dedup (idempotent upserts)
- **Feed creation:** normalize URL → `INSERT ... ON CONFLICT (url) DO NOTHING RETURNING id` (or select existing). Subscription is a separate `user_feeds` upsert. A feed shared by N users has exactly one `feeds` row.
- **Catalog selection:** an admin can attach one editorial catalog entry to a shared feed; users may subscribe to many published catalog feeds in one transaction. Regular accounts start with no `user_feeds` rows and select their own sources. Only ephemeral demo accounts receive the published featured showcase set automatically; creation also maps each demo subscription into a personal folder/tag named after the catalog category, within the same transaction. The catalog never substitutes for arbitrary user URL creation.
- **Personal organization:** a nested personal folder and multi-valued tags update only `user_categories`, `user_feeds`, and `user_feed_categories`; one current folder is presented for each feed, and these changes never enqueue shared work or duplicate shared feeds/articles. Library article drag/drop moves the article's source feed, with the same folder picker available to keyboard and touch users. Folder collapse state persists on the personal category row, and sidebar quick tools edit the selected feed's personal tags/content preference in accessible dialogs.
- **Bookmarklet discovery:** the instance-specific bookmarklet sends only the current public URL back to the authenticated Rusty Feed origin. Rusty Feed performs one SSRF-safe, cookie-free, 2 MiB fetch and bounded no-script alternate-link discovery, then requires an explicit CSRF-protected choice. Confirmation reuses normal global feed upsert/personal subscription semantics, so it cannot create per-user feed copies.
- **Article ingest:** dedup order = **GUID (feed-scoped) → URL (global unique) → content_hash (feed-scoped)**. Insert via `ON CONFLICT (url) DO UPDATE` only when `should_update` (content hash/title/published changed). One row per article regardless of subscriber count. Feed attachments are synchronized under the shared article row lock in stable order; unchanged metadata causes no writes. Attachment-only items use a stable feed-scoped internal URI and are excluded from public-page extraction.

### 7.3 Compute-path dedup (claim + idempotency, O(1) per content version)
The heavy operations — **fetch, extract, embed, summarize** — are keyed by the **shared entity id** and executed at most once per content version:

- **Fetch/refresh** is scheduled **per feed**, not per subscription. The FeedRefresh agent builds the set of *due* feeds (union across all subscribers, deduped) and enqueues one `FetchFeed{feed_id}` job each. Conditional GET (`ETag`/`Last-Modified`) avoids re-downloading unchanged feeds.
- **Extraction/embedding/summarization** are enqueued **per article id**. Before doing work, the agent **claims** the article for that operation:
  - *Extraction:* subscriber preferences are reduced to one shared demand. A public page is fetched when any subscriber requests it and the shared copy is absent or expired; one deduplicated job and a content-revision guard prevent per-user or concurrent duplicate work. RSS and public-page bodies are stored separately.
  - *Embedding:* skip if a row already exists in `article_embeddings`; use `INSERT ... ON CONFLICT (article_id) DO NOTHING` so concurrent workers don't double-compute.
  - *Summary:* first-writer-wins on the shared content revision. On-demand requests are independent of automatic policy. During feed ingest, an enabled singleton `instance_summary_policy` queues only created or materially updated bodies meeting its normalized plain-text character minimum. Public-page extraction repeats the eligibility check against the final effective body; an explicit `SummaryBackfillAgent` action cursor-scans older unsummarized rows in bounded batches. Automatic jobs recheck enablement, threshold, and revision immediately before provider use, so disabling the policy stops already-queued spend. Historical backfill is a separate explicit admin action.
- **Catalog public-content probe:** a published catalog feed may receive one globally deduplicated, background diagnostic keyed by `feed_id`. It tests at most three current shared article URLs and replaces one scalar shared snapshot; it never writes article content or feed policy. The current candidate IDs and a freshness window prevent visitor-driven repeated work.
- **Result:** for a feed/article shared by N users, network + CPU + LLM work per refresh cycle is **O(1)**, not O(N). Only cheap per-user rows (notifications, saved-search matches, read state) scale with N.

### 7.4 Read-path efficiency
- The article list for a user is a join of the shared `articles` with the user's `user_feeds`/categories plus `LEFT JOIN read_articles/bookmarks/feedback` for personal flags. Shared columns are read once; personal flags are per-user.
- Relevance scores are computed per user but **cached** (`score:{user}:{article}`); the shared embedding/summary are **cached once** for all users (`embedding:{article}`, `summary:{article}`).
- The reader selects RSS or the current public-page body from the caller's `user_feeds.settings`. If the requested page is absent/expired, it enqueues extraction immediately, renders RSS without blocking, and may poll the content fragment briefly for the completed shared copy.

### 7.5 Delete/retention safety
- Unsubscribing deletes only `user_feeds` (+ dependent personal rows via cascade). Shared `feeds`/`articles` remain for other subscribers.
- Public-page cleanup uses the longest lifetime requested by current subscribers; an infinite request prevents expiry. Expiry removes only the public-page copy and restores the independently stored RSS body. Article-row retention operates globally, preserves bookmarked articles, and may GC orphan feeds per policy.

---

## 8. The AGENTS Framework (Agentic Background Workflows)

"AGENTS" are the autonomous background workflows that keep content fresh, enriched, and personalized. They are the heart of the dedup guarantee and are designed to be **uniform, observable, coordinated, and extensible**.

### 8.1 The `Agent` abstraction

```rust
/// A unit of autonomous background work. Implementations live in `crates/agents`.
#[async_trait]
pub trait Agent: Send + Sync + 'static {
    /// Stable name used as the durable job kind and in run records.
    fn name(&self) -> &'static str;

    /// Execute one JSON payload and return safe structured run statistics.
    async fn run(
        &self,
        ctx: &AgentContext,
        payload: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value>;
}
```

- **`AgentContext`** currently carries the shared database, `SafeFetchClient`, and immutable shared processing policies. Agents that require an embedder or LLM receive it when the registry is built.
- **Payloads** are bounded JSON persisted in the queue; each agent validates the fields it owns.
- **Outcomes** are safe structured statistics merged into `agent_runs`; raw payloads and unreviewed errors are never rendered to administrators.

### 8.2 Runtime services
1. **Scheduler:** a bounded Tokio loop evaluates validated five-field UTC cron expressions and enqueues deduplicated singleton maintenance and refresh jobs.
2. **Queue:** the custom PostgreSQL `SELECT ... FOR UPDATE SKIP LOCKED` repository is durable and at-least-once, with lane isolation, retries/backoff, `max_attempts`, and dead-letter state.
3. **Singleton lock** (lease): before a singleton run, acquire `agent_locks(name)` via Postgres advisory lock or lease row with `expires_at` (default lease 10 min, matching reference `JOB_LOCK_TIMEOUT`); auto-recover expired leases. Prevents duplicate runs across instances.
4. **Run tracking:** every run writes an `agent_runs` row (`RUNNING`→`SUCCESS/FAILED`, duration, allowlisted stats, bounded sanitized error, trigger). Worker processes heartbeat their configured execution-slot capacity in Postgres, allowing the admin view to distinguish online workers and live jobs from stale queue locks.
5. **Event fan-out:** current writers insert revision-keyed downstream jobs idempotently. The schema reserves `outbox_events`, but its transactional dispatcher and optional Redis nudge remain M7 work. Direct insertion still preserves shared O(1) enrichment.
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
                   │ - upsert articles   │  → revision-keyed downstream jobs
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
         SummaryBackfillAgent → SummarizeArticleAgent (ingest/manual batches or on-demand): first-writer-wins

  Scheduled maintenance agents (singleton):
    • PatternDecayAgent  (decay + prune per-user patterns)      -- was API-only/untracked in ref
    • CleanupAgent       (retention; preserves bookmarks; VACUUM ANALYZE when needed)
    • FeedHealthSweep    (optional; batch notifications and manual-pause maintenance)
    • ClusterStoriesAgent (hourly at :15 UTC; cosine similarity ≥ 0.82 joins a shared story)
    • DailyDigestAgent   (07:00 UTC; the personal **Briefing** — page `/briefing`, with `/digests` and `/digests/refresh` as permanent redirects — built from shared stories; email only when SMTP is configured and the user opted in)
```

Notes:
- **Ordering/reuse:** embedding depends on ingest; saved-search matching depends on embedding (uses cosine) + keyword. Revision-keyed idempotent jobs and transactionally published outbox events sequence these without per-user fan-out.
- **Per-user work is intentional & cheap:** refresh notifications and saved-search matches are inherently per-user, but they operate on already-shared, already-enriched articles.
- **Backpressure:** each agent has bounded concurrency; extraction is additionally throttled per domain (§12).
- **Feed failures:** one queued fetch is one health attempt. One or two failures retain the configured cadence; three unresolved failures become daily checks; after 14 days since the first unresolved failure, checks become weekly. `disabled` remains a manual pause state, not an automatic terminal health transition.
- **Operations activity:** the admin log view is a bounded, no-store projection of structured feed failures and worker-run state. It intentionally omits raw request errors, payloads, and arbitrary agent stats.

### 8.4 Adding a new agent (extensibility)
1. Implement `Agent` in `crates/agents`, define its `Job` payload.
2. Register it with the runtime (name and queue lane/concurrency).
3. Enqueue it through the idempotent job repository, or publish it in the caller's transaction for outbox dispatch when work must commit atomically with content.
4. Runs, locks, safe errors, heartbeats, and admin visibility then use the common runtime.

This uniformity is what lets additional agents, including story clustering and daily briefings, land without touching the framework.

---

## 9. Request Lifecycle & Web Layer

- **Middleware stack (tower):** tracing → timeout/body bounds where applicable → custom opaque DB session → auth extractor → double-submit CSRF on mutations → per-IP authentication rate limit → router. Final response middleware enforces the no-referrer policy.
- **Handlers** extract and validate bounded serde DTOs, call focused domain/repository operations, and render Askama HTML or a safe JSON envelope. Reader fragments are ordinary authenticated HTML endpoints reused by progressive JavaScript.
- **Server-rendered UI:** full pages and standard links/forms provide the baseline; small local JavaScript enhances in-place reader actions, live status, dialogs, drag/drop, copy/install controls, and the resizable panel. Short reader tasks use authenticated, lazily loaded dialogs so they preserve article position without adding destination/configuration queries to every reader render; their ordinary links remain the no-JavaScript path. Personal folder disclosure state is persisted server-side. In side-panel mode the sidebar, article column, and reader are independent viewport-height scroll regions so list scrolling never displaces the reader. Preferences are applied server-side into HTML/CSS variables so there is no client round-trip or theme flash.
- **DTOs & envelopes:** API success `{ "data": ..., "message"?: ... }`, error `{ "error": ..., "details"?: ... }`; status codes per PRD §5 FR-API-2. A single `AppError → IntoResponse` mapping (see §19) ensures consistency and no internal leakage.
- **Progressive enhancement:** core flows work through standard links and form posts; local JavaScript adds convenience without becoming the source of truth. Every page links an instance-branded web manifest with standalone display, platform icons, and Library/Search/Discover shortcuts. Preferences exposes the browser-native install prompt when available and truthful manual home-screen guidance otherwise.
- **Public article shares:** authenticated create/update always rotates a 256-bit opaque token; only its SHA-256 lookup hash and an AEAD-encrypted recoverable copy are stored. A share may expire or be explicitly infinite and can hide title, metadata, the privacy-clean source link, and publisher-served images. Anonymous reads resolve only a non-revoked token, render the selected already-stored sanitized body, and never enqueue extraction or other background work. Missing/expired public-page content falls back to RSS. Responses are `private, no-store`, `noindex`, no-referrer, and script-free under a dedicated CSP.
- **Installed-app cache boundary:** the root-scoped service worker has a closed exact allow-list containing only embedded public CSS, JavaScript, and application icons. It never intercepts navigation, article, API, Live View, media, authentication, or publisher requests. Offline article reading is a separate feature requiring an explicit encrypted/private-data design rather than an accidental cache expansion.

### 9.1 Native and compatibility APIs

Rusty Feed exposes a versioned **native JSON resource API** plus **Google Reader API (GReader)** and **Fever** compatibility adapters so existing RSS clients can interoperate without a second domain model. All three reuse the same repositories and tenant authorization. (PRD FR-API-5.)

- **Design:** the GReader layer is a **thin adapter** mounted at `/accounts/ClientLogin` and `/reader/api/0/*`. It translates protocol shapes to the same repositories used by the native API and UI. Database triggers assign stable compact feed/article aliases once; UUIDs remain canonical and every visible-content query joins through the caller's subscriptions.
- **Fever:** the JSON-only `/fever/` endpoint supports groups, feeds, favicons, items, unread/saved identifiers, and read/save mutations. Users derive a Fever API key locally from `username:password`; Rusty Feed stores only its SHA-256 lookup hash and never stores the password.
- **Native v1:** `/api/v1` covers users, feeds/subscriptions, folders, articles and personal state, and saved searches. It uses bounded cursor pagination, token scopes and revocation, per-token rate limiting, OpenAPI 3.1 documentation, and a token-safe 90-day mutation-attempt audit trail visible to administrators.
- **Policy:** `API_ENABLED`, `GREADER_API_ENABLED`, and `FEVER_API_ENABLED` independently remove each surface with a fail-closed 404 response. Health and metrics remain separate operational endpoints.
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
- **Non-goals:** 100% coverage of every historical GReader or Fever quirk. We target the subset that popular clients actually use (matching Miniflux/FreshRSS behavior), document supported endpoints, and guard it with integration tests that replay complete client request sequences.

### 9.2 Outbound destinations and webhooks

Personal integration profiles reference shared articles but remain user-owned
automation state. Each profile chooses one of 28 capability labels, a bounded
message template/tags/event set, one SSRF-safe endpoint, rate and attempt limits,
and a domain-separated encrypted no-auth/Bearer/Basic/HMAC credential. Explicit
article sends, bookmarks, new feed entries, and completed summaries enqueue a
durable `integration_delivery` job; provider failure never rolls back or blocks
reading/acquisition. Deliveries expose queued/delivering/retrying/delivered/dead
state, exponential backoff, test, pause, credential rotation, and personal
history. HMAC signs the exact versioned JSON body. Discord, Slack, Telegram,
Ntfy, and signed webhooks have native payload shapes; other compatible endpoints
currently receive the documented generic JSON shape, and the parity tracker does
not call all catalog labels turnkey until exact provider contracts are tested.

### 9.3 OPML portability

- `crates/feeds::opml` owns bounded OPML 2.0 parsing and deterministic export;
  it has no database or user-state dependencies. DTDs are rejected, nesting is
  capped at 32 levels, labels/URLs are bounded, and a document may contain at
  most 2,000 subscriptions.
- The web flow is deliberately two-step: file or SSRF-safe URL input produces a
  dry-run preview, then the user selects individual subscriptions or whole
  folder groups. A single batched lookup labels already-subscribed URLs. Existing
  personal names/folders are kept by default or explicitly replaced.
- Shared feeds are still normalized and upserted once. The personal subscription
  plus folder link is applied atomically, and only new subscriptions enqueue the
  deduplicated shared refresh job. Per-entry failures do not discard successful
  imports and are reported without leaking internal errors.
- Export reads only the caller's subscriptions, prefers personal display names,
  rebuilds sibling-scoped folder paths, and returns a private/no-store OPML
  download. JavaScript adds group selection and live counts; individual
  checkboxes and submission continue to work without it.

---

## 10. Caching Strategy

Two-tier cache behind a single `Cache` port, with **shared vs. per-user key separation** (enables cross-user reuse — PRD §6 FR-DEDUP-6).

- **L1 — `moka`** (in-process, always on): microsecond hits for hot shared data (feed metadata, embeddings, summaries, parsed query ASTs). Bounded by entry count/size; TTL per namespace.
- **L2 — Redis** (shared across instances): extends cached values across process restarts. Every key is prefixed by the validated `CACHE_NAMESPACE`, so independent Rusty Feed installations may safely share one Redis service. Optional; if absent or down, L1+DB serve.
- **Pattern:** `cache_get_or_set(key, ttl, || fetch())` (cache-aside), storing only non-null. Write-through where cheap.
- **Invalidation:** tag/prefix based via `SCAN` when Redis is configured (never `KEYS`), plus targeted local deletes. A versioned, namespace-scoped Pub/Sub protocol sends exact-key or prefix invalidations to sibling instances. Each message identifies its sender, is size bounded, and carries no cached value. Redis L2 is enabled only after the initial subscription succeeds; if that subscriber disconnects, the process clears and bypasses L1 until bounded reconnection and resubscription complete. Examples: on feedback → invalidate `score:{user}:*` and `patterns:{user}`; on article update → `*:{article_id}`; on feed refresh → `feed:{feed_id}*`.

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
- Match order: GUID(feed-scoped) → canonical privacy-clean URL(global) → content_hash(feed-scoped). The publisher-provided URL is retained separately for provenance and an explicit personal display choice; cleanup remains one instance policy, never a per-user acquisition variant.
- `should_update` if content_hash changed, title/metadata changed, or `|published_new − published_old| > 60s`; a comments-link-only update retains extracted content, its revision, vectors, and summaries.
- Standard RSS 2 `<comments>` metadata is retained separately from the canonical article URL, so reader views can offer both the original page and its discussion link (for example, Hacker News comments).

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
Effective personal values use the first present of **feed → category → user prefs → system defaults → env** (feed wins). This applies to personal reading and refresh controls. Automatic summarization is intentionally instance-scoped because the result and provider spend are shared: the PostgreSQL singleton overrides its environment seed and applies one enablement/minimum-size decision to every feed. Constraints (admin): refresh 15–1440 min, max_articles 50–5000, max_age 1–365 days; automatic-summary minimum 0–1,000,000 normalized plain-text characters.

### 11.6 Cleanup/retention
- Delete articles older than `max_article_age` **and** trim each feed to newest `max_articles_per_feed`; **preserve bookmarked**. Age basis is explicit/configurable (`published_at` preferred; `createdAt` available for parity). `VACUUM ANALYZE` when deletions exceed a threshold.

---

## 12. Content Extraction & Safe Fetch

- **`SafeFetchClient`** wraps `reqwest` and is the **only** way any code makes outbound HTTP (feeds, article pages, robots.txt, icons, Live View resources, media proxy). It enforces: scheme allow-list (http/https), DNS resolution + **private/link-local/loopback IP blocking** (IPv4 & IPv6, incl. mapped), redirect re-validation (guards DNS-rebinding/redirect-to-internal), size caps, and timeouts. A validated instance policy supplies one publisher-facing user agent and can force HTTP/1.1 to reduce fingerprinting or work around broken HTTP/2 origins. This closes the reference's SSRF gaps (PRD §9.6).
- **Strategies** via an `Extractor` trait + registry with priority + fallback:
  1. **Readability** (`dom_smoothie`): remove publisher chrome, extract main semantic content and metadata, then run Rusty Feed boilerplate cleanup and sanitization. A configurable CSS selector overrides selection; malformed documents retain a density-scored `scraper` fallback. Default.
  2. **Chromium** (`chromiumoxide`, remote CDP, semaphore-bounded): executes inline application rendering against the exact HTML already fetched through `SafeFetchClient`, under an injected no-network CSP, then runs Readability on the rendered DOM. This keeps DNS/redirect/size policy at the SSRF boundary instead of letting a browser navigate independently. The browser receives neither credentials nor network authority; any authorized cookie is applied only by the host-pinned source client before HTML crosses this boundary. Any CDP failure falls back to the local result.
- **Per-domain rate limiting:** shared `domain_extraction_state`; base delay default **3000 ms**, overridable per feed; honor robots.txt `Crawl-delay` (cached 24h/1h-on-error); adaptive `429` backoff **10s→30s→60s→120s** honoring `Retry-After`. Article extraction runs once per shared content revision/lifetime, driven by the union of subscriber requests (§7.3).
- **Catalog availability diagnostic:** the shared catalog snapshot checks at most three current article URLs. It is independent of the feed's source policy, uses no selector or per-feed delay override, always requires a readable robots.txt policy (404 permits), and disables redirects. Robots errors, redirects, rate limits, access statuses, and reader failures are reported as scalar outcomes only; no page HTML or excerpts are stored or rendered by Discover.
- **Personal article view:** each subscription stores one default (`rss`, `article_page`, or `live`) configured from feed-add/settings surfaces or an explicit **Make default for this feed** reader action. The reader's RSS/Page/Live links are temporary request-scoped visualization overrides and never mutate `user_feeds.settings`; next/previous article navigation explicitly drops the override and resolves the destination article's feed default. RSS reads the always-stored shared feed body. A temporary `article_page` request reuses a current shared copy or queues one revision-deduplicated shared extraction with a one-day on-demand lifetime; making it the default contributes the subscription's configured lifetime to aggregate demand. `live` contributes no durable extraction demand: opening an article obtains the existing short-lived article-scoped ticket and displays the SSRF-fetched publisher document inside the ordinary reader body through a dynamic-viewport-height, credentialless opaque-origin sandbox whose document scrolls internally, with RSS as its no-JavaScript/error fallback. Items without a safe public URL retain the feed's Live selection but render an explicit unavailable state over RSS rather than silently appearing to switch modes. Neither temporary nor permanent selection duplicates a feed, article, extraction, embedding, or summary.
- **Content normalization, manipulation, and sanitization:** feed media types are honored before storage: HTML is cleaned, Markdown is rendered with safe GFM structure, and plain text is escaped into paragraphs/line breaks instead of being misread as markup. RSS 2 FeedBurner `origLink` metadata is resolved locally before the ordinary entry link, avoiding a redirect while preserving publisher provenance. Extracted publisher HTML first loses navigation, forms, share/newsletter/recommendation blocks, and duplicate page titles. At most 20 administrator-owned ordered declarative rules may remove CSS-selected nodes or replace selected text with a bounded regex; sanitization always runs again afterward. Immutable rule versions restore as a new version. A change clears that feed's conditional validators, invalidates its requested public-page processing versions, and queues one shared rebuild (FND-179). Personal include/exclude regex filters are evaluated only at the subscription visibility boundary and never mutate shared ingestion. A deterministic no-network pixel policy then removes explicit tiny/CSS-hidden images, bounded administrator host rules, known tracker hosts/beacons, and optional strict URL signals before **`ammonia`** applies the semantic tag/attribute allow-list, resolves safe links/responsive media, and removes scripts. Separate RSS/public-page removal counts are stored on the one shared article and the matching count is selected at read time; a personal preference controls only indicator visibility. The instance URL policy removes a bounded built-in set of analytics parameters plus validated exact/prefix additions from shared article keys, extraction/Live View targets, comments, and surviving content links; named domains can be excepted where query signatures are functional. Original top-level URLs remain stored for provenance and a clean/original user preference. The same sanitized DOM converts to compact portable Markdown for LLM/text consumers, preserving headings, lists, links, tables, images, quotations, and code without sending raw publisher tags. A shared processing version makes legacy page copies—including infinite-lifetime copies—eligible for one deduplicated refresh on access after extraction rules improve; cleanup never runs per user/render.
- **YouTube playback:** the reader detects a bounded set of YouTube URLs already present in shared article metadata/content without fetching the video page. Before sanitization removes active publisher iframes, a validated official YouTube embed is reduced to a canonical inert watch link; arbitrary iframes remain removed. Processing version 3 refreshes legacy shared public-page copies, and detection unions the selected body with stored RSS/page variants so the same video appears in standalone, inline, and side-panel readers regardless of visualization. Instance-approved source hosts are validated at startup. Linked offsets are parsed with bounded, overflow-safe rules, retained on the canonical fallback, and translated to the player's bounded `start` parameter. The default user mode renders an explicit on-demand iframe from fixed `youtube-nocookie.com`; an administrator may additionally configure one named credential-free HTTPS Invidious-compatible base whose exact origin is added to `frame-src`. Link-only mode never creates an iframe. The reader exposes idle, connecting, ready/error, unload, and reload states; unload removes `src` immediately and restores consent. Ordinary publisher/alternative requests remain no-referrer, while the built-in player alone uses YouTube's required `strict-origin-when-cross-origin` policy so it receives the Rusty Feed origin but never the article path. Users choose the validated provider, autoplay-after-consent, and truthful caption preference; each subscription may inherit, force the built-in privacy player, or reduce to links. These are personal render choices only and never duplicate shared acquisition. Optional duration enrichment is separately disabled by default, daily-budgeted, source-size bounded, cache-limited, and keyed once by validated video id.
- **Publisher access profiles:** anonymous public acquisition remains the default. An administrator may assign one shared profile to a feed for authorized access: explicit host pins, an optional path pin, encrypted cookie and/or encrypted explicit proxy, and an optional invalid-certificate exception. Cookies require a path and 1–365 day expiry; certificate exceptions require expiry. Proxy redirects are disabled, proxy DNS has a separate connection pin, and private publisher targets remain blocked. Profiles are audited without secret values and cannot create per-user variants or legitimize paywall/access-control bypass. Each shared feed can explicitly respect or ignore robots.txt for normal public-page extraction; the catalog diagnostic always uses the anonymous client and respects it.

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
- **Local default:** `fastembed` runs BAAI BGE-small-en-v1.5 as a normalized 384-dimension ONNX model. It downloads once into `MODEL_CACHE_DIR`; `rusty-feed-cli fetch-model` provides an explicit online preparation step and that cache can be copied to an air-gapped host. The legacy deterministic hash model remains available only when explicitly selected for lightweight tests or emergency lexical fallback.
- **OpenAI-compatible:** the focused `reqwest` client batches requests and requires the configured provider to return the instance's fixed 384 dimensions.
- **Text embedded:** `title + "\n\n" + excerpt + "\n\n" + content[..2000]`. Batched (configurable size, default 10; streaming batch 25). Idempotent insert (skip if present) → O(1) across users.

### 13.3 Query path
- Interactive search supports keyword, semantic, and hybrid modes. Keyword mode uses the indexed vectors in §6.3 and works before embedding. Semantic mode embeds the query with the instance model → pgvector KNN (`ORDER BY embedding <=> $1 LIMIT k`) with HNSW → normalize similarity → optional recency blend. Hybrid mode unions bounded candidates from both indexes and exposes the independently normalized components with the final blend so the result is explainable. If query embedding is unavailable, the web path visibly falls back to PostgreSQL keyword results instead of failing core search.
- Instance defaults are mode `hybrid`, text configuration `simple`, and 60% semantic weight. A user can inherit or override all three; an individual search may override only the mode. Shared article vectors/embeddings remain O(1), while preferences remain per-user.

---

## 14. LLM Integration & Secrets

- **Provider configuration today:** summary requests resolve the enabled **user → admin/system → environment** OpenAI-compatible/Ollama cascade and continue to the next profile after a sanitized provider failure. User endpoints are fixed to official OpenAI; only administrators can supply compatible endpoints. The embedding provider remains one instance-wide space by design.
- **Summarization** (`SummarizeArticleAgent` / on-demand / automatic): sanitized article HTML is converted to structured Markdown before prompt construction; the prompt returns JSON `{summary, key_points[], topics[], sentiment?}`; robust JSON extraction with text fallback; content is truncated to a safe max; results are stored **once** on the shared article revision (first-writer-wins). Automatic generation is controlled by `instance_summary_policy`, initialized once from `LLM_AUTO_SUMMARIZE` and `LLM_AUTO_SUMMARIZE_MIN_CHARS` (default 2,000 normalized plain-text characters), then editable live under Administration → AI providers. Feed ingest queues eligible created/materially updated revisions directly, extraction applies the same gate to the final body, and explicit historical backfill uses bounded durable batches.
- **Durable cost tracking (fix):** every billable call appends to **`cost_ledger`** (user, provider, model, kind, tokens, `cost_usd`, timestamp). Admin/user views aggregate by day/month/provider. Replaces the reference's in-process counters (PRD §9.3). Pricing table includes `gpt-4o-mini` (the common default) to avoid the reference's mis-pricing bug.
- **Secrets (unified AEAD — fix):** stored LLM, integration, and acquisition-profile credentials use **XChaCha20-Poly1305**; the 32-byte key is derived from `ENCRYPTION_SECRET` via **HKDF-SHA256** with a stable domain-separated context per credential class. One versioned format authenticates the key version as associated data: `base64(version ‖ nonce ‖ ciphertext ‖ tag)`. Secrets are never logged or redisplayed; masking is used where presence must be shown. (Replaces the reference's two incompatible schemes; §21 covers migration.)

---

## 15. Authentication & Authorization

- **OAuth2/OIDC** (implemented directly over `reqwest`; FND-117): Google, GitHub, and a configurable generic OIDC provider; authorization-code flow with PKCE (S256) + `state` CSRF (both stashed in short-lived HTTP-only cookies between redirect and callback); profile fetched from userinfo; account linked in `oauth_accounts` (find-by-subject → match-by-email → create).
- **Local accounts:** email + password hashed with **Argon2id**; optional email verification; registration mode (open/invite/closed) via admin/env; **the first persistent, non-demo user becomes ADMIN** if none exists (self-host bootstrap). Ephemeral demo and read-only demo-admin rows never close registration or consume that bootstrap slot.
- **Passkeys:** optional `webauthn-rs` passwordless authentication with a pure HTTPS origin (HTTP only for loopback development), multiple named public credentials, password confirmation for local-account enrollment or a session created within 15 minutes for passwordless/OAuth enrollment, recent-session revocation, one-use five-minute server-side ceremony state, signature-counter/backup-state updates, and a last-credential recovery guard. No biometric material is stored. The library's attestation verifier uses platform OpenSSL; application HTTP remains rustls-only (FND-177/FND-178).
- **Trusted reverse proxy:** disabled by default. A verified email/name header is considered only when Axum's immediate TCP peer belongs to an explicitly parsed CIDR; forwarded client-address headers never grant trust. Local sessions take precedence and coexist for recovery. Auto-provision, exact domain/admin mappings, and upstream logout are bounded startup policy; roles become database-owned after creation.
- **Sessions:** a small, dependency-light DB-backed scheme (FND-116) — the browser holds an opaque random token in a secure, HTTP-only, SameSite=Lax cookie; only its SHA-256 hash is stored in `sessions`, so a DB read never exposes live tokens. CSRF is a double-submit cookie token for cookie-auth mutations.
- **Authorization:** role checks (`admin`, `admin_readonly`, `user`, `guest`) via extractors; every data query is **scoped by `user_id`** for per-user tables; shared-content reads require an active subscription where appropriate. Admin GET routes accept `admin_readonly` (read-only view); every admin **write** requires full `admin` (structural 403 for read-only admins).
- **Rate limiting**: a bounded in-process per-key limiter (`crates/web/src/ratelimit.rs`) covers auth, native API, feed-add/discovery, search, summarize, OPML, media proxy, Fever, and GReader; limits are configurable (`0` == unlimited). Forwarded client-IP headers are honored only from trusted-proxy CIDRs.
- **Referrer isolation:** the outer router middleware overwrites every response—including static assets, errors, and unmatched-route fallbacks—with `Referrer-Policy: no-referrer`. Sanitized content links and explicit external-link templates also carry `rel="noopener noreferrer"` and `referrerpolicy="no-referrer"`; publisher-supplied link policy cannot weaken these attributes.

---

## 16. Configuration

- **Source:** environment variables (12-factor) layered over an optional TOML file named by `RUSTY_FEED_CONFIG_FILE` via **`figment`**; `.env` is supported in development (`dotenvy`) without overriding an existing process variable. Parsed into a **typed, validated `AppConfig`** at startup; the process refuses to boot on invalid/missing required config. The exhaustive operator reference is `docs/CONFIGURATION.md`, and `rusty-feed-cli config-check` reports resolved modes/presence without printing URLs or secret values.
- **Key groups** (superset of reference): database URL; Redis URL/password + `CACHE_ENABLED` and a 1–64 character ASCII `CACHE_NAMESPACE` (`rusty-feed` by default; make it unique when installations share Redis); auth (`SESSION_SECRET`, OAuth client IDs/secrets, provider URLs, `REGISTRATION_MODE`, `PASSKEYS_ENABLED`, `PASSKEY_RP_NAME`, and the disabled-by-default `REVERSE_PROXY_AUTH_*` trust boundary); APIs (`API_ENABLED`, `GREADER_API_ENABLED`, `FEVER_API_ENABLED`); installed-app branding (`PWA_NAME`, `PWA_SHORT_NAME`, `PWA_THEME_COLOR`, `PWA_BACKGROUND_COLOR`); UI customization (`CUSTOM_CSS_ENABLED`, disabled-by-default `CUSTOM_JAVASCRIPT_ENABLED`, byte limit); public sharing (`PUBLIC_ARTICLE_SHARING_ENABLED`, bounded default/maximum finite lifetime, and explicit infinite-link policy); embeddings (`EMBEDDINGS_ENABLED` master switch — `false` skips loading/downloading a model and forces every dependent knob keyword-only; `EMBEDDING_PROVIDER`, `EMBEDDING_MODEL`, `MODEL_CACHE_DIR`, batch sizes, `EMBEDDING_AUTO_GENERATE`); search (`SEARCH_MODE`, `SEARCH_TEXT_CONFIG`, `SEARCH_SEMANTIC_WEIGHT_PERCENT`); LLM (`LLM_PROVIDER`, models, `OPENAI_API_KEY`, `OPENAI_BASE_URL`, `OLLAMA_BASE_URL`); extraction/acquisition (`EXTRACTION_*`, `CHROMIUM_ENABLED`/endpoint, `EXTRACTION_DELAY_MS`, `RESPECT_ROBOTS_TXT`, validated `OUTBOUND_USER_AGENT`, `OUTBOUND_HTTP1_ONLY`; sensitive per-feed acquisition profiles live encrypted in PostgreSQL); URL privacy (`TRACKING_URL_CLEANUP_ENABLED`, bounded cleanup/exception rules); tracking-image privacy (`TRACKING_PIXEL_*`); YouTube privacy/duration (`YOUTUBE_*`); media proxy (`MEDIA_PROXY_ENABLED`, per-object maximum, aggregate cache budget, positive/negative TTLs, and the bounded passive-only `MEDIA_PROXY_EXTRA_INLINE_MIME_TYPES` extension list); jobs (`ENABLE_CRON_JOBS`, schedules/concurrency/locks); crypto (`ENCRYPTION_SECRET`); server (`BIND_ADDR`, `BASE_URL`, `LOG_LEVEL`, `SYSTEMD_NOTIFY_ENABLED`).

### 16.1 Live publisher isolation

Live View is an on-demand, server-mediated rendering path rather than a direct publisher iframe. Each open first requests a fresh, short-lived article/user ticket encrypted from the instance session secret; the credentialless iframe then redeems it without receiving session cookies. Publisher HTML and static resources are fetched only through `SafeFetchClient`, size capped, cached under shared keys, and served with an opaque-origin CSP sandbox. Per-resource single-flight locks and short negative caching collapse concurrent cache misses, preserving shared O(1) publisher work. Resource URLs are individually ticketed and URL-bound, so the endpoint cannot be used as a general authenticated proxy. `allow-same-origin` and top navigation are never granted. If the current publisher document cannot be fetched, the iframe receives the stored sanitized article copy.

### 16.2 Shared reader media

Attachment players use `/articles/{article_id}/media/{position}` by default, and a privacy-proxied article header uses `/articles/{article_id}/hero`. Neither route accepts a caller-supplied URL: each authorizes an active subscription, resolves stored shared metadata, fetches through `SafeFetchClient`, and verifies passive inline MIME types; file attachments are sent as `application/octet-stream` with `Content-Disposition: attachment`. Hero images use an isolated source-keyed cache namespace and the smaller of the configured object limit or 8 MiB. Successful bodies and short-lived failure categories are keyed once in PostgreSQL, independent of subscribers; per-process single-flight locks and a PostgreSQL advisory lock collapse concurrent misses across every instance. Single byte ranges are served from the shared body for browser seeking. Positive/negative lifetimes, per-object bytes, aggregate cache bytes, whole-instance enablement, and a bounded passive-only MIME extension list are environment-configured; active document types can never be admitted. Daily cleanup removes expired rows and successful writes evict the oldest bodies over budget. Users choose global defaults and may inherit/reset or override proxy-all, proxy-HTTP-only, direct delivery, on-demand, metadata, or links-only behavior per subscription; the same effective delivery choice governs the article header image.

---

## 17. Observability & Operations

- **Logging/tracing:** `tracing` supports JSON or pretty output and `tower-http::TraceLayer` supplies HTTP spans. Agent runs persist bounded allowlisted statistics and sanitized actionable errors; a per-run captured-log ring buffer remains optional future depth.
- **Metrics:** the `metrics` Prometheus exporter is available at `/metrics`. HTTP count and latency labels use bounded route templates, methods, and status codes rather than user-controlled paths. Queue/worker state and provider costs are additionally queryable from durable operational tables; deeper per-stage histograms can be added with the feature that owns them.
- **Health:** `/api/health` is liveness. `/api/health/ready` requires PostgreSQL and reports Redis as connected or L1-only; LLM/Chromium diagnostics remain to be added without making optional dependencies fatal.
- **systemd lifecycle:** both server and standalone worker auto-detect `NOTIFY_SOCKET` when `SYSTEMD_NOTIFY_ENABLED=true`, send `READY=1` only after database/migrations and listener/worker startup, send `STOPPING=1` before draining, and honor `WATCHDOG_USEC` at half the manager interval. Notification failures are warnings, never reasons to stop otherwise healthy service operation.
- **Admin ops:** a wide, auto-refreshing queue view shows online workers/slots, due versus delayed work, live versus stalled claims, dead letters, per-kind depth, recent completions, and allowlisted outcomes without exposing payloads or raw errors. User roles, feeds, catalog, costs, public-share policy/activity, and safe feed operations are available; cache clear, VACUUM/ANALYZE, embedding maintenance, provider management, and richer storage controls remain M13 gates.
- **Backups:** documented `pg_dump`/restore; Redis is disposable.

---

## 18. Deployment Topology

- **Default (single-node, self-hosted):** one server process **with embedded worker** (`RUN_EMBEDDED_WORKER=true`) + Postgres 17/pgvector. Redis is optional L2 caching and the current extractor does not require headless Chromium; Compose can start only the app and database for this mode.
- **Scaled:** N stateless `rusty-feed-server` replicas (no embedded worker) behind a load balancer + M `rusty-feed-worker` replicas; all share Postgres, and may share Redis for L2 cache. Agents coordinate through database leases and the Postgres queue, so scheduled jobs run once and queue work is load-balanced.
- **Images:** a multi-stage non-root runtime image and separate Chromium service exist. Signed amd64/arm64 publication, size verification, and upgrade tests remain release gates.
- **Migrations:** applied by an entrypoint/CLI step (`rusty-feed-cli migrate`) before serving; safe/idempotent; opt-in auto-run.
- **Assets:** curated CSS, dependency-free JavaScript, icons, and the service worker are embedded through `rust-embed` for a self-contained server artifact.

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
- **Browser E2E:** Playwright with Chromium starts the actual Rust server on an isolated port and exercises the accessibility viewport matrix, catalog/OPML, reader isolation, Live View, consent-gated media, priority summaries, drag/drop, RTL locale persistence, recoverable customization/Safe Mode, no-JavaScript fallbacks, and authorization boundaries.
- **Property tests** (`proptest`) for the query parser and dedup invariants.
- **Load/bench** (`criterion` for hot functions; a small harness for search/list p95 and refresh throughput) to guard PRD §7 targets.
- **CI gates:** `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, locked Playwright/Chromium browser E2E, migration check, and a smoke `docker compose --env-file .env -f docker/compose.yaml up` health probe.

---

## 21. Migration and upgrades

Rusty Feed upgrades are performed in place: back up PostgreSQL, stop writers, deploy
the new binary or image, and run `rusty-feed-cli migrate` before restarting. All 41
migrations are additive or explicitly data-preserving, and rollback is by
restoring the pre-upgrade database backup with the previous binary.

The supported migration path from NeuReed is OPML export/import. It preserves
subscriptions and nested folder names while Rusty Feed rebuilds shared articles,
extractions, and embeddings once for every subscriber. API credentials are
entered again so they are encrypted directly with Rusty Feed's single AEAD format;
there is no legacy-secret conversion command that could silently misinterpret
old ciphertext. A direct database converter may be added later only with
fixture-backed validation for IDs, personal state, embeddings, and both legacy
secret formats.

Rusty Feed and NeuReed must not write to the same database. The complete upgrade,
host-move, backup, and NeuReed OPML procedures are in `docs/MIGRATION.md`.

---

## 22. Key Design Decisions (ADR summary)

| # | Decision | Rationale | Trade-off |
|---|---|---|---|
| 1 | **Axum + Askama + local progressive JavaScript** (no SPA) | Low memory, fast, server-owned state, and no runtime frontend dependency | Rich interactions require carefully tested local enhancements |
| 2 | **SQLx runtime queries** over an ORM | pgvector raw-SQL fit, low overhead, and hermetic builds; integration tests validate SQL against PostgreSQL | Less "magic"; more explicit SQL and test responsibility |
| 3 | **Custom Postgres-backed job queue** (Redis optional) | Durable, inspectable shared work with no extra broker required | Queue throughput is bounded by PostgreSQL; bounded lanes and the transactional outbox keep behavior explicit |
| 4 | **One embedding space per instance** (single fixed-dim table + model guard) | Eliminates mixed-dim corruption; simplest correct design; one HNSW index | Model change requires an explicit re-embed/migration |
| 5 | **Unified XChaCha20-Poly1305 secrets** | One secure scheme; fixes dual-format debt | One-time secret migration required |
| 6 | **Server+worker one codebase, deploy 1 or 2 ways** | Simple by default, scalable when needed | Must keep worker/server wiring clean (DI at bin level) |
| 7 | **Centralized `SafeFetchClient`** | Uniform SSRF protection on every egress | All fetch paths must route through it (enforced by layering) |
| 8 | **UUIDv7 PKs** | Time-sortable, index-friendly, distributed-safe | Differs from reference's cuid (handled in migration) |
| 9 | **GReader-compatible API adapter** (native API + GReader layer) | Interop with the existing RSS client ecosystem; no duplicate business logic | Must track a de-facto (un-versioned) protocol; scoped to the popular subset |
| 10 | **Download embedding model on first run** (volume cache) | Small image; offline after first fetch | First run needs network; cold cache adds one-time download |

---

*See `docs/PRD.md` for requirements and success metrics, and `README.md` for the project guide and setup.*
