# RustSS &nbsp;·&nbsp; `rustss`

> Your RSS reader, but it actually understands what you read — and it runs as a single fast binary you own.

**RustSS** is a self-hosted, multi-user, AI-augmented RSS/Atom reader. It learns what each user cares about and surfaces it using **semantic (vector) search**, **per-user personalization**, and optional **LLM summarization** — while staying lean enough to run on a home server or a small VPS.

This repository (`rustss`) is a **ground-up reimplementation in Rust** of the original [`neureed`](https://github.com/madpin/neureed) (a Next.js 16 / TypeScript app). The goal: the same (and eventually more) capabilities, with a **fraction of the memory footprint**, higher throughput, and dramatically simpler operations.

> **Status: 📐 Design phase.** The foundational documents (PRD, Architecture) are complete and awaiting review. No application code has been written yet — see the [Roadmap](#-roadmap). Setup instructions below describe the **target** experience.

---

## ✨ Vision

RSS is the best way to own your information diet — but classic readers drown you in noise. RustSS makes the feed *smart*:

- **🧠 It learns you.** Thumbs up/down and natural reading behavior train a per-user model; articles you'll care about float to the top.
- **🔍 Search by meaning.** "machine learning tutorials for beginners" finds the right articles even when they never use those words (pgvector embeddings).
- **📌 Saved searches that never sleep.** Persistent boolean + semantic queries auto-match new articles and can notify you on high-relevance hits.
- **📝 Optional AI summaries.** TL;DR + key points + topics per article, using OpenAI, a local model, or your own endpoint — with real cost tracking.
- **🤝 Multi-user by design.** A feed followed by 50 people is fetched, extracted, embedded, and summarized **once** — your reading state stays private.
- **🔐 Yours.** Self-hosted, privacy-first, single binary, boring to operate.

---

## 🎯 Why RustSS?

The original `neureed` is feature-rich and proven — its data model and algorithms are our source of truth. RustSS keeps that behavior while re-architecting the runtime for the things self-hosters feel every day:

| | Original (`neureed`) | RustSS |
|---|---|---|
| Runtime | Node.js 24 / Next.js 16 | Rust + Tokio (single binary) |
| UI | React SPA (SSR + client) | Server-rendered HTML + HTMX + Alpine |
| Idle memory (app) | Hundreds of MB | **Target < 100 MB** |
| Background jobs | node-cron in-process | Durable Postgres-backed **AGENTS** queue |
| Vector storage | mixed dims in one column ⚠️ | **space-tagged, correct** per model |
| Costs / secrets | in-memory / two crypto formats | **durable ledger** / one AEAD scheme |
| Deploy | Node + build | Compose stack, multi-arch, `< 150 MB` image |

It also fixes a set of real deficiencies found in the reference (unwired conditional GET, embedding-space corruption, SSRF gaps in extraction, no-op "preserve starred", ephemeral cost tracking). See **[docs/PRD.md §9](docs/PRD.md)**.

---

## 🧩 Feature overview

**Core (v1 — feature parity):**
- RSS 2.0/1.0, Atom, and JSON Feed parsing; conditional GET; smart dedup (GUID → URL → content hash)
- Content extraction (Readability + headless Chromium) with per-domain rate limiting, robots.txt, and SSRF protection
- Semantic search + "related articles" (pgvector, HNSW); optional recency-weighted ranking
- Per-user personalization (TF keyword patterns, sigmoid relevance scoring, time-decay)
- Saved searches with a boolean+semantic DSL (`AND`/`OR`/`NOT`/phrases/groups) and match notifications
- LLM summaries (summary + key points + topics) with a provider cascade and cost tracking
- Feeds & subscriptions: personal categories (nestable/collapsible), tags, bulk operations, OPML import/export
- Feed health tracking (healthy/warning/error/disabled, auto-disable, error log)
- Three reading modes (side panel / inline / standalone), 14 themes, deep typography & display preferences
- In-app notifications; admin dashboard (stats, jobs, storage, providers, users)
- OAuth (Google/GitHub/Generic) **and** local accounts (Argon2id)

**Planned (v1.1+ / v2):** bookmarks/favorites + smarter retention, "seen" state, news **clustering + LLM digests**, configurable keyboard shortcuts, i18n, ADHD-friendly reading formatting, media/YouTube embeds, storage/log admin tooling. Full list in **[docs/PRD.md §8](docs/PRD.md)**.

---

## 🏗️ How it works (30-second tour)

```
Browser (HTMX) ─▶ Axum server (Askama HTML / JSON API)
                        │
      ┌─────────────────┼─────────────────┐
      ▼                 ▼                 ▼
 PostgreSQL         Redis (L2)      Local ONNX / LLM
 + pgvector       cache · pub/sub      providers
 (source of truth,
  job queue)
      ▲
      │ SKIP LOCKED / apalis
 AGENTS worker: FeedRefresh ▶ FetchFeed ▶ Extract ▶ Embed ▶ SavedSearchMatch
                Summarize · PatternDecay · Cleanup · (v2) Cluster/Digest
```

**The golden rule:** *content is shared, state is personal.* Feeds, articles, embeddings, and summaries are stored and computed **once**; reading state, feedback, learned patterns, and saved searches are per-user. A feed shared by *N* users costs **O(1)** fetch/extract/embed/summarize work, not O(N). See **[docs/ARCHITECTURE.md §7](docs/ARCHITECTURE.md)**.

---

## 🚀 Getting started (target experience)

> ⚠️ These commands describe the **planned** developer/operator workflow. They are not yet functional — the codebase is in the design phase.

### Run it (Docker Compose — recommended)

```bash
git clone https://github.com/madpin/rustss.git
cd rustss
cp .env.example .env          # set SESSION_SECRET, ENCRYPTION_SECRET, OAuth/keys as needed
docker compose up -d          # app + postgres/pgvector + redis + headless chromium
open http://localhost:8080
```

The first account you register becomes the **admin**. New users are auto-subscribed to a curated set of default feeds.

### Develop locally

```bash
# Prereqs: Rust (stable), Docker, Tailwind standalone CLI
docker compose up -d postgres redis chromium   # dependencies only
cp .env.example .env

cargo run -p rustss-cli -- migrate            # apply DB migrations
cargo run -p rustss-server                    # start the server (embedded worker by default)

# Useful:
cargo run -p rustss-worker                    # run AGENTS separately (scaled mode)
cargo run -p rustss-cli -- create-admin       # bootstrap an admin
cargo test                                     # unit + integration tests
cargo clippy -D warnings && cargo fmt --check  # lint & format
```

### Configuration (essentials)

```bash
DATABASE_URL=postgres://rustss:rustss@localhost:5432/rustss
REDIS_URL=redis://localhost:6379
SESSION_SECRET=<32+ random bytes>
ENCRYPTION_SECRET=<32+ random bytes>         # encrypts API keys & feed cookies

# Auth (pick at least one; local accounts work with none)
GOOGLE_CLIENT_ID=... GOOGLE_CLIENT_SECRET=...
GITHUB_CLIENT_ID=... GITHUB_CLIENT_SECRET=...
REGISTRATION_MODE=open                        # open | invite | closed
```

The full configuration reference will live in `docs/configuration.md`. Env vars are typed and validated at startup — the app refuses to boot on invalid config.

---

## 📚 Documentation

| Document | What's inside |
|---|---|
| **[docs/PRD.md](docs/PRD.md)** | Product requirements: personas, user flows, functional/non-functional requirements, the multi-user dedup mandate, performance targets, milestones, and fixes to the reference |
| **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)** | System design: full tech stack + rationale, data model, the **AGENTS** framework, caching, algorithms & constants, security, deployment, testing, and migration |
| **AGENTS.md** *(coming with code)* | Build/test/run commands and contributor conventions |
| **docs/MIGRATION.md** *(coming)* | Migrating data from the original `neureed` |

---

## 🗺️ Roadmap

- [x] **Phase 1** — Deep analysis of the original `neureed` codebase
- [x] **Phase 2** — Foundational docs: PRD, Architecture, README *(this document)*
- [ ] **Review & approval** of the three documents ← *we are here*
- [ ] **Phase 3** — Implementation
  - [ ] Workspace scaffold, config, error types, CI (fmt/clippy/test)
  - [ ] Database schema + migrations (multi-user shared/personal model)
  - [ ] Core domain + repositories (SQLx)
  - [ ] Feed fetch/parse/dedup + SafeFetchClient (SSRF)
  - [ ] AGENTS framework + FeedRefresh/FetchFeed pipeline
  - [ ] Content extraction (Readability + Chromium) + rate limiting
  - [ ] Embeddings + semantic search (space-tagged vectors)
  - [ ] Personalization (patterns, scoring, feedback)
  - [ ] Saved searches (DSL, matcher, notifications)
  - [ ] LLM summarization + durable cost ledger
  - [ ] Auth (OAuth + local), sessions, preferences
  - [ ] HTMX UI (reading, feeds mgmt, search, admin)
  - [ ] Deployment (Docker Compose, images, docs)
- [ ] **v1.1** — bookmarks/favorites, "seen" state, hybrid search, OPML fixes
- [ ] **v2** — clustering + digests, keyboard shortcuts, i18n, ADHD reading mode

Work is committed iteratively per milestone using **[Conventional Commits](https://www.conventionalcommits.org/)** (`feat:`, `fix:`, `docs:`, `refactor:`, `chore:`), with commit bodies explaining the *why*.

---

## 🙏 Credits

- Based on the original **[NeuReed](https://github.com/madpin/neureed)** by [@madpin](https://github.com/madpin).
- Built on the shoulders of the Rust and PostgreSQL/pgvector communities.

## 📜 License

MIT — see [LICENSE](LICENSE).
