# Rusty Feed &nbsp;·&nbsp; `rusty-feed`

> Your RSS reader, but it actually understands what you read — and it runs as a single fast binary you own.

**Rusty Feed** is a self-hosted, multi-user, AI-augmented RSS/Atom reader. It learns what each user cares about and surfaces it using **semantic (vector) search**, **per-user personalization**, and optional **LLM summarization** — while staying lean enough to run on a home server or a small VPS.

This repository (`rusty-feed`) is a **ground-up reimplementation in Rust** of the original [`neureed`](https://github.com/madpin/neureed) (a Next.js 16 / TypeScript app). The goal: the same (and eventually more) capabilities, with a **fraction of the memory footprint**, higher throughput, and dramatically simpler operations.

> **Status: 🧪 v1 release candidate.** All fourteen implementation milestones are complete. The expanded Miniflux audit is **65/67 complete**, with full-string localization and turnkey contracts for every advertised hosted integration still partial. The repository has not been tagged or published from this worktree; see the [Roadmap](#-roadmap), [release evidence](docs/RELEASE_EVIDENCE.md), and [parity tracker](docs/MINIFLUX_PARITY.md).

---

## ✨ Vision

RSS is the best way to own your information diet — but classic readers drown you in noise. Rusty Feed makes the feed *smart*:

- **🧠 It learns you.** Thumbs up/down and natural reading behavior train a per-user model; articles you'll care about float to the top.
- **🔍 Search by meaning.** "machine learning tutorials for beginners" finds the right articles even when they never use those words (pgvector embeddings).
- **📌 Saved searches that never sleep.** Persistent boolean + semantic queries auto-match new articles and can notify you on high-relevance hits.
- **📝 Optional AI summaries.** TL;DR + key points + topics per article, using OpenAI, a local model, or your own endpoint — with real cost tracking.
- **🤝 Multi-user by design.** A feed followed by 50 people is fetched, extracted, embedded, and summarized **once** — your reading state stays private.
- **🔐 Yours.** Self-hosted, privacy-first, single binary, boring to operate.

---

## 🎯 Why Rusty Feed?

The original `neureed` is feature-rich and proven — its data model and algorithms are our source of truth. Rusty Feed keeps that behavior while re-architecting the runtime for the things self-hosters feel every day:

| | Original (`neureed`) | Rusty Feed |
|---|---|---|
| Runtime | Node.js 24 / Next.js 16 | Rust + Tokio (single binary) |
| UI | React SPA (SSR + client) | Askama HTML + progressive local JavaScript |
| Idle memory (app) | Hundreds of MB | **Target < 100 MB** |
| Background jobs | node-cron in-process | Durable Postgres-backed **AGENTS** queue |
| Vector storage | mixed dims in one column ⚠️ | **space-tagged, correct** per model |
| Costs / secrets | in-memory / two crypto formats | **durable ledger** / one AEAD scheme |
| Deploy | Node + build | Compose stack, multi-arch, `< 150 MB` image |

It also fixes a set of real deficiencies found in the reference (unwired conditional GET, embedding-space corruption, SSRF gaps in extraction, no-op "preserve starred", ephemeral cost tracking). See **[docs/PRD.md §9](docs/PRD.md)**.

---

## 🧩 Feature overview

**Implemented today:**
- RSS 2.0/1.0, Atom, and JSON Feed parsing; conditional GET; smart dedup (GUID → URL → content hash)
- RSS fallback or opt-in public-page extraction with per-domain rate limiting, robots.txt, SSRF protection, safe rich-content rendering, and administrator-owned host/path-pinned profiles for explicitly authorized cookie/proxy/certificate cases
- Semantic search + "related articles" (pgvector, HNSW); optional recency-weighted ranking
- Per-user personalization (TF keyword patterns, sigmoid relevance scoring, time-decay)
- Saved searches with a boolean+semantic DSL (`AND`/`OR`/`NOT`/phrases/groups) and match notifications
- On-demand and optional automatic LLM article summaries with a reserved low-latency queue, live progress, user → administrator → environment provider fallback, encrypted managed credentials, actionable errors, and durable cost tracking
- Feed magazine: 200 verified starter sources across news, technology, research, science, security, public-interest, and culture; automatic country/category/language filtering, observed content/cadence/last-post stats, multi-select subscriptions, and unrestricted custom feed URLs
- Personal feed organization: stable hierarchical folder navigation, one folder per feed, an Unfiled scope, multi-valued tags, bulk move/tag/refresh actions, drag-to-folder organization, downloaded source icons, and a reduced sidebar mode
- OPML 2.0 file/URL import with preview, selective nested-folder mapping, duplicate policy and partial results, plus hierarchy-preserving export
- Feed health tracking (healthy/warning/error/manual-disabled): normal cadence for 1–2 failures, daily after 3, weekly after 14 failing days, resolved structured error history, and last-successful-update timestamps
- Three reading modes (side panel / inline / standalone), opt-in sandboxed live publisher view, original and RSS-comment links, themes, deep typography/display preferences, and arrow-key-first shortcuts
- Installable home-screen/desktop experience with instance branding and quick Library/Search/Discover shortcuts; only public application assets are cached
- Admin dashboard: instance stats, users/roles, shared feed inventory/cadence, feed magazine editing, live worker/queue status, safe operations logs, public-share activity, and AI costs
- Revocable public article links with finite/infinite lifetimes and configurable RSS/public-page, metadata, source-link, and image policies
- OAuth (Google/GitHub/Generic) **and** local accounts (Argon2id)
- A versioned native JSON API and the commonly used GReader client subset, with stable protocol IDs and replay conformance tests
- Fever-compatible mobile-client API, passwordless WebAuthn passkeys, trusted reverse-proxy auth, and scoped app credentials
- Personal entry filters, versioned shared rewrite rules with preview/rollback/reprocessing, and opt-in host-pinned cookie/proxy/self-signed-certificate acquisition profiles
- Versioned personal CSS, administrator-managed opt-in JavaScript with Safe Mode, public YouTube duration enrichment, and 20 selectable locale/format profiles
- Named article destinations and signed event webhooks with encrypted credential rotation, templates, event filters, low-impact retries, dead letters, and delivery history

**After v1:** the product program continues with news **clustering + LLM digests**, ADHD-friendly reading formatting, completion of every translated UI string and accessibility label, and exact turnkey contracts for the remaining hosted integration providers. Fever compatibility, YouTube duration enrichment, automatic/custom HTTPS with HTTP/2, touch navigation, and a validated alternative-player path are already implemented. Full status lives in **[docs/DEVELOPMENT_PLAN.md](docs/DEVELOPMENT_PLAN.md)** and the separate parity tracker.

The living Miniflux feature-parity and customization checklist is in
**[docs/MINIFLUX_PARITY.md](docs/MINIFLUX_PARITY.md)**.

---

## 📸 Screenshots

A smart reading list with **per-feed unread counts**, in-place reading, and one-click AI summaries:

![Library](docs/screenshots/library.png)

| Discover magazine (filter-aware select-all) | Standalone reader |
|---|---|
| [![Discover feeds](docs/screenshots/discover.png)](docs/screenshots/discover.png) | [![Article reader](docs/screenshots/article.png)](docs/screenshots/article.png) |

| Admin instance overview | Shared-feed admin (stats · filters · pagination) |
|---|---|
| [![Admin overview](docs/screenshots/admin-overview.png)](docs/screenshots/admin-overview.png) | [![Admin shared feeds](docs/screenshots/admin-feeds.png)](docs/screenshots/admin-feeds.png) |

| Feeds & bulk actions | AI providers | Integrations |
|---|---|---|
| [![Feeds](docs/screenshots/feeds.png)](docs/screenshots/feeds.png) | [![AI providers](docs/screenshots/ai-providers.png)](docs/screenshots/ai-providers.png) | [![Integrations & webhooks](docs/screenshots/integrations.png)](docs/screenshots/integrations.png) |

<sub>Regenerate against a running instance with `node scripts/capture-screenshots.mjs` — see [docs/screenshots/README.md](docs/screenshots/README.md).</sub>

---

## 🏗️ How it works (30-second tour)

```
Browser (HTML + progressive JS) ─▶ Axum server (Askama HTML / JSON API)
                        │
      ┌─────────────────┼─────────────────┐
      ▼                 ▼                 ▼
 PostgreSQL         Redis (L2)       Embed / LLM
 + pgvector          cache             providers
 (source of truth,
  job queue)
      ▲
      │ SKIP LOCKED
 AGENTS worker: FeedRefresh ▶ FetchFeed ▶ Extract ▶ Embed ▶ SavedSearchMatch
                Summarize · PatternDecay · Cleanup
```

**The golden rule:** *content is shared, state is personal.* Feeds, articles, embeddings, and summaries are stored and computed **once**; reading state, feedback, learned patterns, and saved searches are per-user. A feed shared by *N* users costs **O(1)** fetch/extract/embed/summarize work, not O(N). See **[docs/ARCHITECTURE.md §7](docs/ARCHITECTURE.md)**.

---

## 🚀 Getting started

**➡️ Full step-by-step guide: [docs/GETTING_STARTED.md](docs/GETTING_STARTED.md)** — covers localhost (Docker Compose *and* from source), Dokploy, and Proxmox, with no Rust knowledge assumed.

Quick version (Docker Compose):

```bash
git clone https://github.com/madpin/rusty-feed.git && cd rusty-feed
cp .env.example .env      # set SESSION_SECRET and ENCRYPTION_SECRET (openssl rand -base64 48)
docker compose --env-file .env -f docker/compose.yaml up -d
curl http://localhost:8080/api/health
```

> **Status:** v1 release candidate — the application is usable end to end, its
> shared-content invariants and browser flows are tested, and release evidence
> is recorded. A maintainer still needs to create the intended `vX.Y.Z` tag to
> publish signed artifacts and multi-architecture images.

### Run it (Docker Compose — recommended)

```bash
git clone https://github.com/madpin/rusty-feed.git
cd rusty-feed
cp .env.example .env          # set SESSION_SECRET, ENCRYPTION_SECRET, OAuth/keys as needed
docker compose --env-file .env -f docker/compose.yaml up -d  # full stack
open http://localhost:8080
```

The first account you register becomes the **admin**. New users can choose from 200 curated magazine sources with country and language filters, or add their own feed URL.

### Redis-free single-process mode

Redis is optional. `rusty-feed-server` embeds the background worker by default, so a
small single-node deployment needs only PostgreSQL 17 with pgvector:

```bash
CACHE_ENABLED=false rusty-feed-server
```

Set `DATABASE_URL`, `SESSION_SECRET`, and `ENCRYPTION_SECRET` first. SQLite is
not yet a supported database backend.

### Develop locally

A self-documenting Make command assistant wraps setup, Cargo, Docker, databases,
tests, debugging, and maintenance. Start with:

```bash
make help       # list every command and supported variable
make setup      # initialize the development environment once
make dev        # run dependencies, migrations, and the server
make verify     # formatting, Clippy, and database-backed Rust tests
```

The underlying commands remain available directly:

```bash
# Prereqs: Rust (stable) and Docker. Node 20 is needed only for browser E2E tests.
cp .env.example .env
docker compose --env-file .env -f docker/compose.yaml up -d postgres
# Optional Redis L2 cache and bounded Chromium extraction fallback:
# docker compose --env-file .env -f docker/compose.yaml up -d redis chromium

cargo run -p rusty-feed-cli -- migrate            # apply DB migrations
cargo run -p rusty-feed-server                    # start the server (embedded worker by default)

# Useful:
cargo run -p rusty-feed-worker                    # run AGENTS separately (scaled mode)
cargo test                                     # unit + integration tests
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all --check
```

The first registered persistent (non-demo) account becomes the administrator. `create-admin`,
`fetch-model`, `reindex`, `backfill`, `migrate`, and `config-check` support
bootstrap and maintenance workflows; use `--help` for their current arguments.

### Browser E2E tests

```bash
npm ci
docker compose --env-file .env -f docker/compose.yaml exec -T postgres \
  createdb --username rusty_feed rusty_feed_e2e
PLAYWRIGHT_DATABASE_URL=postgres://rusty_feed:rusty_feed@localhost:5435/rusty_feed_e2e \
  PLAYWRIGHT_REDIS_URL=redis://localhost:6379 \
  npm run test:e2e
```

Create the dedicated database once; an "already exists" response on later runs is harmless. Do not point Playwright at the database used by Rust integration tests or a real Rusty Feed instance. Playwright starts an isolated Rust server on `127.0.0.1:8088` with its embedded worker disabled and the deterministic test embedding model selected. It exercises discovery and OPML flows, reader modes and typography, Live View, privacy-gated media/YouTube, public sharing, feed organization and drag/drop, keyboard shortcuts, the fixed side panel, live AI-summary progress, admin queue refresh, no-JavaScript fallbacks, and authorization boundaries. CI creates a fresh `rusty_feed_e2e` database, installs Chromium, and runs this suite after Rust tests.

### Full-stack stress test

The ignored stress suite drives concurrent shared-feed subscriptions, deduplicated queue workers, feed ingestion, extraction, embeddings, mocked summaries, saved-search matching, L1/optional Redis caching, authenticated HTML/native API reads and writes, health, metrics, and tenant-isolation checks. Run it only against a dedicated disposable database:

```bash
TEST_DATABASE_URL=postgres://rusty_feed:rusty_feed@localhost:5435/rusty_feed_stress \
  TEST_REDIS_URL=redis://localhost:6379 \
  npm run test:stress
```

The suite always enforces correctness and prints throughput plus HTTP p95 latency. Set `STRESS_ENFORCE_TARGETS=1` to enforce the PRD performance targets. Workload controls are `STRESS_USERS`, `STRESS_FEEDS`, `STRESS_ARTICLES_PER_FEED`, `STRESS_REQUESTS`, `STRESS_CONCURRENCY`, and `STRESS_SUMMARIES`. Local feed/article and LLM servers keep the run deterministic; Chromium and real external providers are intentionally outside this test.

### Configuration (essentials)

```bash
DATABASE_URL=postgres://rusty_feed:rusty_feed@localhost:5432/rusty_feed
# Optional Redis L2 cache. For L1-only, omit REDIS_URL and use CACHE_ENABLED=false.
REDIS_URL=redis://localhost:6379
CACHE_ENABLED=true
CACHE_NAMESPACE=rusty-feed                    # make unique when Redis is shared by installations
SESSION_SECRET=<32+ random bytes>
ENCRYPTION_SECRET=<32+ random bytes>         # encrypts stored API credentials

# Auth (pick at least one; local accounts work with none)
GOOGLE_CLIENT_ID=... GOOGLE_CLIENT_SECRET=...
GITHUB_CLIENT_ID=... GITHUB_CLIENT_SECRET=...
REGISTRATION_MODE=open                        # open | invite | closed
```

See the complete [configuration reference](docs/CONFIGURATION.md), including
TOML layering and the secret-safe `rusty-feed-cli config-check` diagnostic. Rusty Feed
refuses to boot with missing or invalid required configuration.

---

## 📚 Documentation

| Document | What's inside |
|---|---|
| **[docs/GETTING_STARTED.md](docs/GETTING_STARTED.md)** | Step-by-step install for localhost (Docker Compose or from source), Dokploy, and Proxmox — beginner friendly |
| **[docs/PRD.md](docs/PRD.md)** | Product requirements: personas, user flows, functional/non-functional requirements, the multi-user dedup mandate, performance targets, milestones, and fixes to the reference |
| **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)** | System design: full tech stack + rationale, data model, the **AGENTS** framework, caching, algorithms & constants, security, deployment, testing, and migration |
| **[docs/API.md](docs/API.md)** | Versioned native JSON resources, bearer authentication, scopes, and examples |
| **[docs/CONFIGURATION.md](docs/CONFIGURATION.md)** | Every runtime and Compose setting, precedence, validation, security notes, and safe diagnostics |
| **[docs/USER_GUIDE.md](docs/USER_GUIDE.md)** | Complete guide to reading, organizing feeds, search, summaries, APIs, customization, and administration |
| **[docs/OPERATIONS.md](docs/OPERATIONS.md)** | Production health, backup/restore, upgrades, queues, retention, and failure recovery |
| **[docs/MIGRATION.md](docs/MIGRATION.md)** | Rusty Feed upgrades, NeuReed OPML migration, and moving hosts |
| **[docs/RELEASE_EVIDENCE.md](docs/RELEASE_EVIDENCE.md)** | Reproducible performance, resilience, model, accessibility, and restore evidence |
| **[docs/DEVELOPMENT_PLAN.md](docs/DEVELOPMENT_PLAN.md)** | Evidence-backed milestone status, remaining acceptance work, and delivery order |
| **[docs/MINIFLUX_PARITY.md](docs/MINIFLUX_PARITY.md)** | Living 67-item Miniflux parity and customization tracker |
| **[docs/FINDINGS.md](docs/FINDINGS.md)** | Resolved defects, deliberate deviations, and open engineering risks |
| **[AGENTS.md](AGENTS.md)** | Build/test/run commands and canonical contributor conventions |

---

## 🗺️ Roadmap

The detailed status and acceptance evidence are maintained in
[docs/DEVELOPMENT_PLAN.md](docs/DEVELOPMENT_PLAN.md).

- [x] **Phase 1** — Deep analysis of the original `neureed` codebase
- [x] **Phase 2** — Foundational docs: PRD, Architecture, README *(this document)*
- [x] **Phase 2 review** — requirements, architecture, and contributor contract accepted as living documents
- [x] **Phase 3 — v1 implementation** *(14 of 14 milestones complete)*
  - [x] Shared-content ingestion, durable workers/outbox, production local embeddings, safe Chromium fallback, search/personalization, summaries, native/GReader APIs, and user/admin experiences
  - [x] Invitations and session controls, feed health/pause/bulk actions, retention, queue/storage operations, managed AI providers, strict CSP, and responsive accessibility matrix
  - [x] Performance, dependency-failure, model offline-cache, upgrade/backup/restore, and browser acceptance evidence
  - [x] Versioned native archives, Debian/RPM packages, and multi-registry amd64/arm64 images with checksums, SBOM/provenance, smoke/upgrade checks, and keyless signatures (publication occurs when a maintainer pushes a version tag)
- [ ] **Broader Miniflux parity program** — 65/67 complete; complete catalogs for every UI/validation/notification/accessibility string and exact service-specific adapters for every advertised integration remain
- [ ] **Post-v1 product program** — clustering/digests and ADHD reading mode remain deliberately scoped after v1

Work is committed iteratively per milestone using **[Conventional Commits](https://www.conventionalcommits.org/)** (`feat:`, `fix:`, `docs:`, `refactor:`, `chore:`), with commit bodies explaining the *why*.

---

## 🙏 Credits

- Based on the original **[NeuReed](https://github.com/madpin/neureed)** by [@madpin](https://github.com/madpin).
- Built on the shoulders of the Rust and PostgreSQL/pgvector communities.

## 📜 License

MIT — see [LICENSE](LICENSE).
