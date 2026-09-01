# AGENTS.md — Working in the RustSS repo

Guidance for AI coding agents (and humans) contributing to **RustSS**, a self-hosted, multi-user, AI-augmented RSS reader written in Rust. This is a ground-up Rust reimplementation of the original `neureed` (Next.js/TypeScript).

> This file is the **canonical** contributor guide. `CLAUDE.md`, `.cursorrules`, and `GEMINI.md` are thin pointers to it — keep this one authoritative and up to date.

---

## 0. The one rule you must never break

**Content is shared; state is personal.** Feeds, articles, embeddings, and summaries are stored and computed **once** and shared across all subscribers. Per-user data (subscriptions, read/seen/bookmarks, feedback, learned patterns, saved searches, preferences) is layered on top. A feed followed by *N* users must cost **O(1)** fetch/extract/embed/summarize work — never O(N).

If a change would fetch/extract/embed/summarize per-user, or duplicate a feed/article row, it is wrong. See `docs/ARCHITECTURE.md` §7 and `docs/PRD.md` §6.

---

## 1. Start here

Read these before coding (in order):
1. `docs/PRD.md` — what we're building and why (requirements, targets).
2. `docs/ARCHITECTURE.md` — how it's built (stack, data model, AGENTS framework, algorithms/constants).
3. `docs/DEVELOPMENT_PLAN.md` — the milestone-by-milestone plan; pick up the current milestone.
4. `docs/MINIFLUX_PARITY.md` — living feature-parity and customization status; update it with parity work.
5. `docs/FINDINGS.md` — known smells/bugs/risks. **Add to it** whenever you find something.

---

## 2. Tech stack (quick reference)

- **Runtime:** Rust (stable) + Tokio. **Web:** Axum + tower/tower-http. **UI:** Askama templates + embedded curated CSS + dependency-free progressive JavaScript (server-rendered; no SPA).
- **DB:** PostgreSQL 17 + pgvector via **SQLx** (runtime-checked SQL). **Cache:** moka (L1) + optional Redis (L2).
- **Background work (AGENTS):** custom durable Postgres `SKIP LOCKED` queue, bounded lane-isolated workers, UTC cron schedules, transactional outbox dispatch, lease/advisory singleton guards, run records, and heartbeats.
- **Feeds/extract:** `feed-rs`, `reqwest` (rustls), `dom_smoothie` + `scraper` fallback, bounded remote `chromiumoxide`, `ammonia`.
- **AI:** production local fastembed/BGE embeddings with guarded one-model space, explicit hash fallback for tests/emergencies, and OpenAI-compatible HTTP clients.
- **Auth:** local accounts (`argon2`) + custom DB-backed sessions (opaque token cookie, hash in `sessions`) + double-submit CSRF + per-IP rate limiting; OAuth (Google/GitHub/generic OIDC) via direct `reqwest` (PKCE + state). **Crypto:** XChaCha20-Poly1305 + HKDF-SHA256.
- **Errors:** `thiserror` (libs) + `anyhow` (bins). **Config:** `figment`. **Obs:** `tracing` + `metrics`.

Full rationale: `docs/ARCHITECTURE.md` §3.

---

## 3. Repository layout

```
crates/
  core/    domain types, config, errors, shared policies (no app-specific I/O)
  db/      SQLx repositories + migrations + pgvector
  cache/   moka L1 + redis L2
  feeds/   SafeFetchClient + feed-rs parse + dedup
  extract/ readability + bounded Chromium fallback + rate limiter + robots + sanitize
  embed/   deterministic local + OpenAI embedders (single active space)
  llm/     OpenAI/Ollama clients + summarization + cost meter
  search/  semantic search, saved-search DSL/matcher, scoring, patterns
  agents/  AGENTS framework + concrete agents
  web/     Axum app: routers, handlers, Askama templates, auth, DTOs
bins/
  server/  HTTP server (optionally embeds the worker)
  worker/  standalone AGENTS runtime
  cli/     migrate, create-admin, fetch-model, reindex, backfill, config-check
crates/web/templates/  Askama HTML
crates/web/static/     embedded CSS, JavaScript, icons, and service worker
docker/     Dockerfile, compose.yaml, entrypoint.sh
docs/       PRD, ARCHITECTURE, DEVELOPMENT_PLAN, MINIFLUX_PARITY, FINDINGS
```

**Layering rule:** `core` depends on nothing app-specific; domain crates depend on `core`; `agents`/`web` depend on domain crates; `bins` wire everything (DI at startup). Never reach sideways into another adapter's internals.

---

## 4. Commands

> These commands are available in the current workspace. See `docs/DEVELOPMENT_PLAN.md` for the remaining milestone acceptance work.

```bash
# Build / lint / format / test  (run before every commit)
cargo build
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test                      # workspace unit + integration tests
cargo test -p <crate>           # a single crate

# Browser E2E (dev-only Node dependency; starts RustSS on :8088 with worker off)
npm ci
docker compose --env-file .env -f docker/compose.yaml exec -T postgres \
  createdb --username rustss rustss_e2e
PLAYWRIGHT_DATABASE_URL=postgres://rustss:rustss@localhost:5435/rustss_e2e \
  PLAYWRIGHT_REDIS_URL=redis://localhost:6379 \
  npm run test:e2e

# Dependencies (local dev)
docker compose --env-file .env -f docker/compose.yaml up -d postgres redis chromium

# Database (SQLx)
cargo run -p rustss-cli -- migrate          # apply migrations (needs DATABASE_URL)
# DB integration tests run when TEST_DATABASE_URL is set, else they skip:
#   docker compose --env-file .env -f docker/compose.yaml up -d postgres
#   TEST_DATABASE_URL=postgres://rustss:rustss@localhost:5432/rustss cargo test  # use POSTGRES_PORT

# Run
cargo run -p rustss-server                  # server (embedded worker by default)
cargo run -p rustss-worker                  # standalone AGENTS worker
# The first registered account becomes admin. create-admin/fetch-model/reindex/
cargo run -p rustss-cli -- demo-reset --confirm   # wipe all demo accounts (public launch §7)

# Frontend CSS/JS/icons are embedded directly via rust-embed.

# Full stack
docker compose --env-file .env -f docker/compose.yaml up -d # app + postgres + redis + chromium
```

**Env:** copy `.env.example` → `.env` and pass it to Compose with `--env-file .env`. Required: `DATABASE_URL`, `SESSION_SECRET`, `ENCRYPTION_SECRET`. See `docs/ARCHITECTURE.md` §16 for all groups.

---

## 5. Conventions

- **Errors:** `thiserror` enums in libraries; `anyhow` only in bins/agent boundaries. No `unwrap()`/`expect()` on fallible paths outside tests and startup. Map to HTTP via the single `web::AppError: IntoResponse`.
- **SQL:** use **runtime** `sqlx::query`/`query_as` (not the compile-time macros) so builds stay hermetic (no DB or `.sqlx/` needed); SQL is validated by integration tests against a real Postgres (FND-107). Shared writes are **idempotent** (`ON CONFLICT`). Per-user reads/writes are **always scoped by `user_id`**. `FromRow` structs live in `rustss-db` with column-list constants kept in sync with the schema.
- **IDs:** UUIDv7. **Time:** `time` crate + `timestamptz`. **Vectors:** the `pgvector` crate; one fixed dimension per instance (`EMBEDDING_MODEL`); never pad/truncate across dimensions (FND-002).
- **Outbound HTTP:** only via `SafeFetchClient` (SSRF-guarded). No direct `reqwest` calls to feed/article/robots/media URLs (FND-006).
- **Secrets:** encrypt with the unified AEAD helper; never log secret material; use the masking helper for display.
- **Caching:** cache-aside via the `Cache` port; **shared vs per-user** key separation; invalidate with `SCAN`/tags, never `KEYS` (FND-008).
- **Comments:** don't add/remove unrelated comments. Match surrounding style. Document public items with `///`.
- **Background work:** every job is an `Agent` run through the framework (scheduler/queue/lock/run-record). Don't spawn ad-hoc `tokio::spawn` loops for recurring work (FND-007).
- **Tests:** unit-test algorithms against the exact constants in `docs/ARCHITECTURE.md` §11; integration-test repos/agents against ephemeral Postgres; mock HTTP with `wiremock`.

---

## 6. Definition of Done (every change)

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean
- [ ] `cargo test` green (including the dedup invariant tests)
- [ ] `npm run test:e2e` green for browser-facing changes
- [ ] New env/config in `.env.example` + `docs/ARCHITECTURE.md` §16
- [ ] DB integration tests pass with `TEST_DATABASE_URL` set (dedup invariants stay green)
- [ ] New smells/risks/bugs recorded in `docs/FINDINGS.md` (reference the `FND-*` id in the commit)
- [ ] `docs/MINIFLUX_PARITY.md` status/evidence updated when a parity capability changes
- [ ] Conventional Commit with a body explaining **why** (see §7)

---

## 7. Git & commits

- **Conventional Commits:** `feat:`, `fix:`, `docs:`, `refactor:`, `chore:`, `test:`, `perf:`, `build:`, `ci:`. Scope by crate where useful, e.g. `feat(db): ...`.
- Commit body explains **why**, not just what. Reference `FND-*` when addressing a finding.
- Commit iteratively per logical milestone (see `docs/DEVELOPMENT_PLAN.md`). Prefer several small commits over one giant one.
- Do **not** push unless explicitly asked. Do **not** commit secrets. Never weaken security controls to make CI pass — escalate instead.

---

## 8. Gotchas & things to preserve

- **Verified constants over docs:** where the reference's own docs disagreed with its code, we use the **code-verified** values (see `docs/FINDINGS.md` FND-014). Trust `docs/ARCHITECTURE.md` §11.
- **One embedding model per instance:** changing `EMBEDDING_MODEL` requires an explicit re-embed/migration; the store guards against dimension/model mismatch.
- **Enrichment is decoupled from fetch:** extraction/embedding/summarization are agents keyed by the shared article id, not per-user (that's how we keep O(1)).
- **GReader API:** it's a de-facto, unversioned spec; target the Miniflux/FreshRSS-supported subset and keep the replay conformance tests green (FND-105).
- **Graceful degradation:** Redis/LLM/Chromium being down must not break core reading/search.

---

## 9. Status

The fourteen-milestone v1 implementation is complete and under release-candidate validation. The broader Miniflux audit is 65/67 complete; full translated-string coverage and exact turnkey adapters for every advertised hosted integration remain partial. `docs/DEVELOPMENT_PLAN.md` is the v1 milestone source of truth, `docs/RELEASE_EVIDENCE.md` records acceptance results, and `docs/MINIFLUX_PARITY.md` is the extended-capability source of truth. Update all applicable documents whenever reality changes.

**Public launch (open registration):** the `docs/PUBLIC_LAUNCH_PLAN.md` phases are implemented — AI cost/compute budgets + metering (FND-187), per-user quotas (FND-188), ingest fan-out caps (FND-189), rate-limit coverage + trusted client IP (FND-190), signup anti-abuse (email verification, CAPTCHA, lockout, password hygiene — FND-191), content moderation/takedown + feed blacklist (FND-192), charset-correct extraction (FND-193), stored-body caps (FND-194), legal surfaces + data export/delete (FND-195), a demo sandbox + read-only admin, and doc reconciliation (FND-196). Flip `REGISTRATION_MODE=open` only after setting the public-tier config in `docs/CONFIGURATION.md` (budgets, quotas, SMTP, CAPTCHA, retention).
