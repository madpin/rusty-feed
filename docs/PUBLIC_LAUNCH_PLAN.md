# RustSS — Public Launch Plan (Open Registration)

**Status:** Proposal / planning document — not yet implemented
**Author:** Platform team
**Created:** 2026-08-31
**Companion docs:** `docs/PRD.md`, `docs/ARCHITECTURE.md`, `docs/DEVELOPMENT_PLAN.md`, `docs/FINDINGS.md`, `docs/OPERATIONS.md`

---

## 0. Why this document exists

RustSS v1 is a complete, release-candidate **self-hosted** reader. Its own PRD lists a
*"public multi-tenant SaaS with billing/quotas"* as an explicit **non-goal** (`docs/PRD.md` §2.2),
and the whole design assumes **1–500 trusted users** on one node (`docs/PRD.md` §7.2).

Opening the instance to **public, anonymous, open registration** changes the threat model
from *"a small group I trust"* to *"the adversarial internet."* The engineering is largely
sound (SSRF defense, sanitization, dedup, encrypted secrets, durable jobs are all strong), but
the product was never asked to defend its **wallet, its storage, its shared content tables, or
its signup form** against strangers. That is the gap this plan closes.

This document also specifies the features you asked for:
1. A **feed blacklist** (with a shipped default list) — §5
2. **RSS content cleaning** improvements — §6
3. A **demo mode** (shared guest sandbox) that **resets every 24h**, incl. a **read-only admin** view — §7
4. **Per-user LLM summaries with your own API key**, surfaced in preferences (already built — needs surfacing + a cost policy) — §8.1
5. A **marketing landing page** ("what we do") with **screenshots** and highlighted features — §8.2
6. A **RustSS vs Miniflux** comparison on the landing page — §8.3
7. A **first-run setup wizard** so new users pick layout (side-by-side/…), theme & colors, and typography, **with a real-time preview** (screenshot fallback) — §8.4

### Decisions locked for this plan (your input)
| Area | Decision |
|---|---|
| Demo mode | **Shared guest sandbox** on the same instance as real accounts; ephemeral demo users, reset daily; includes a **read-only admin** view so demo users can "see as an admin" without changing anything (§7.6) |
| AI posture | **Local embeddings + LLM summaries, gated by strict global + per-user budgets and rate limits** |
| Signup friction | **Full**: email verification + CAPTCHA + per-user quotas + tightened rate limits (adds an SMTP dependency) |
| Feed blacklist | **Admin domain + URL block, plus a curated default blocklist** shipped and updatable |

---

## 1. TL;DR — what must happen before you flip `REGISTRATION_MODE=open` to the world

**Hard blockers (do not launch without these):**
1. **AI cost enforcement** — meter embeddings (they are currently *not* metered at all) and enforce global + per-user daily budgets *before* every paid call. (§4.1, FND-187)
2. **Per-user quotas** — cap feeds, saved searches, categories, API tokens, integrations, shares. (§4.2, FND-188)
3. **Ingest fan-out cap** — bound how many articles one feed refresh may create/enqueue. (§4.3, FND-189)
4. **Rate-limit coverage + a trustworthy client-IP** — throttle feed-add, search, summarize, OPML, media-proxy, Fever, and GReader; stop trusting `X-Forwarded-For` unconditionally. (§4.4, FND-190)
5. **Signup anti-abuse** — turn on the dormant email verification, add CAPTCHA, add per-account lockout. (§4.5, FND-191)
6. **Content moderation & takedown** — because *content is shared*, one abuser's article is everyone's article; you need a global purge + abuse-report path. (§4.6, FND-192)
7. **Legal surfaces** — Terms, Privacy Policy, an abuse/DMCA contact, and GDPR export/delete. (§9, FND-195)
8. **Feed blacklist** — the requested moderation primitive. (§5)

**Strongly recommended before launch:** content-cleaning hardening (§6), demo mode (§7), a real marketing landing page + BYO-key AI (§8), abuse observability (§10), finite retention default, and doc-drift cleanup (§11).

Everything below is prioritized, cites the exact code that needs to change, and is grouped into shippable phases (§12).

---

## 2. The invariant that governs every change here

> **Content is shared; state is personal.** (`AGENTS.md` §0, `docs/PRD.md` §6)

This rule has two sharp consequences for a *public* instance that it did not have for a private one:

- **Abuse is amplified through the shared tables.** A malicious/illegal feed or article is stored **once** and is visible to enrichment, discovery, and potentially other users. Moderation must therefore act on **shared** rows (feeds/articles), not just on the offending user's subscription. (See §4.6 and §5.)
- **Reset/deletion must respect the boundary.** The demo reset (§7) must delete **per-user** rows for demo users and must **never** delete shared seed content or any real user's data. Deleting a subscription must not orphan-delete shared content others rely on (already true: `FR-DEDUP-5`).

Every task in this plan is annotated where it touches this invariant.

---

## 3. Current-state audit (evidence)

What already exists (and is good) vs. what is missing. All line references verified in-tree.

### 3.1 Already strong — do not rebuild
- **SSRF defense** on every outbound hop, incl. redirects and DNS: `crates/feeds/src/safe_fetch.rs` (`is_blocked_ip` :53, `SafeResolver` :91, `validate_url` :333, redirect re-validation :223).
- **HTML sanitization**, applied on ingest + extraction + render (defense in depth): `crates/core/src/html.rs:135` (`sanitize_article_html_with_policies`), hardened links/images :406. iframes/scripts/styles stripped.
- **Tracking removal**: URL params `crates/core/src/tracking_url.rs`, pixels `crates/core/src/tracking_pixel.rs`; YouTube consent-gating `crates/core/src/youtube.rs` + `crates/web/static/app.js`.
- **Fetch/size/timeout bounds**: feed body 10 MiB (`crates/feeds/src/lib.rs:41`), extraction 5 MiB (`crates/extract/src/lib.rs:45`), robots 512 KiB, 30s timeout, 5-redirect cap.
- **Encrypted secrets** (XChaCha20-Poly1305 + HKDF), **durable LLM cost ledger** (`crates/db/src/cost_ledger.rs`), **account deletion cascades** (per-user rows all `ON DELETE CASCADE`).
- **Registration modes** open/invite/closed + one-use invitations (`crates/core/src/config.rs:35`, `crates/web/src/auth/handlers.rs:197`), CSRF, Argon2id, PKCE OAuth.
- **Admin feed pause** (`health_status='disabled'`) enforced in 5 refresh paths (`crates/db/src/feeds.rs:72,114,127`, `crates/agents/src/fetch_feed.rs:103`, `crates/db/src/jobs.rs:334`).

### 3.2 Missing / dangerous under open registration
| Gap | Evidence | Finding |
|---|---|---|
| **Embeddings never metered; no LLM/embedding budget** | `cost_ledger` used only in `crates/llm/src/lib.rs:331` + admin display; **absent** from `crates/embed`, `crates/search`. No "budget" concept except YouTube/media. | FND-187 (critical) |
| **On-demand summarize is unthrottled & unbudgeted** | `POST /articles/{id}/summarize` `crates/web/src/reader.rs:1359` — no limiter, no budget check. | FND-187 |
| **No per-user quotas** | `subscriptions::subscribe` `crates/db/src/subscriptions.rs:18`, `saved_searches::create` :10, `categories::create` :31 — no count checks. | FND-188 (high) |
| **No cap on articles per feed ingest** | `refresh_feed` loops all entries `crates/feeds/src/lib.rs:275`; `parse.rs:461` no limit. A 10 MiB feed can fan out huge row + job counts. | FND-189 (high) |
| **Rate limiting is narrow & spoofable** | Only `auth_limiter`/`api_limiter` (`crates/web/src/state.rs:269`). Feed-add, search, summarize, Fever, most GReader, media-proxy unthrottled. Client IP trusts `X-Forwarded-For` blindly (`crates/web/src/extract.rs:19`). Limiter is in-process only. | FND-190 (high) |
| **No signup anti-abuse** | `email_verified_at` column exists (`crates/db/src/users.rs:10`) but is **never written or checked**. No CAPTCHA, no per-account lockout. Password rule is only `len < 8` (`auth/handlers.rs:256`). | FND-191 (high) |
| **No global content takedown / abuse reporting** | Admin can pause a feed but there is no "purge this shared article everywhere" action and no user report flow. | FND-192 (high) |
| **Extraction ignores charset** | `String::from_utf8_lossy` in `crates/extract/src/lib.rs:458` (+ live_view, youtube_duration); non-UTF-8 pages corrupt. | FND-193 (medium) |
| **No stored-body size cap after sanitize** | Only fetch-time caps exist; a large-but-under-cap body is stored/rendered for everyone. | FND-194 (medium) |
| **No legal surfaces / data export** | No ToS/Privacy/abuse pages; no full per-user data export (OPML export exists, data export does not). | FND-195 (medium) |
| **Doc drift** | `docs/ARCHITECTURE.md` claims `tower_governor`; code uses a custom limiter. FND-001 marked `planned` but shipped in M6. | FND-196 (low) |

> Add FND-187…196 to `docs/FINDINGS.md` §B when work starts (reference the id in each commit, per `AGENTS.md` §6). Proposed rows are collected in §13.

---

## 4. Release blockers — abuse, cost & safety

### 4.1 AI cost & compute enforcement (FND-187, **critical**)

You chose *local embeddings + LLM with budgets*. Local embeddings are "free" of API cost but **not free of CPU** (every article ingest and every semantic search embeds text on-box), and LLM summaries cost real money. Today nothing stops a stranger from summarizing thousands of distinct articles or hammering search.

**Design**
1. **Meter embeddings.** Add cost/usage recording to `EmbedArticleAgent` (`crates/agents/src/embed_article.rs`) and interactive query embedding (`crates/web/src/search.rs:117`). Even for the local provider, record a **compute unit** count (articles/queries embedded) so budgets and dashboards work regardless of provider. Reuse the `cost_ledger` shape (`crates/db/src/cost_ledger.rs`) or add a sibling `usage_ledger`.
2. **Budgets in config + admin settings** (new):
   - `AI_LLM_GLOBAL_DAILY_USD` (instance kill-switch; e.g. `5.00`)
   - `AI_LLM_PER_USER_DAILY_USD` (e.g. `0.10`)
   - `AI_LLM_PER_USER_DAILY_CALLS` (e.g. `25`)
   - `AI_EMBED_PER_USER_DAILY_CALLS` (search + ingest attribution; e.g. `500`)
   - Demo users get a separate, tighter tier (§7.4).
3. **Check before spend.** Before any LLM call in `crates/llm/src/lib.rs:summarize_article` and before query embedding in search, consult the ledger for today's usage and reject with a friendly `AppError::RateLimited`/`QuotaExceeded` when over. Record **after** success as today (`crates/llm/src/lib.rs:331`).
4. **Keep `LLM_AUTO_SUMMARIZE=false`** for public (it is already the default, `crates/core/src/config.rs:333`). On-demand summaries only, behind the per-user cap + a new summarize rate limit (§4.4).
5. **Provider-key blast radius.** If an admin/env LLM key is set, strangers spend *your* budget. The per-user + global caps above are what make this safe. Document this explicitly.
6. **The scalable answer is bring-your-own-key (§8.1).** Users with their own enabled key spend their *own* quota and are **exempt from the instance budget** (still rate-limited for job/DB abuse). On a public instance, prefer keeping instance-funded summaries off or on a tiny global budget and steering motivated users to add a key.

**Invariant note:** summaries/embeddings are shared, first-writer-wins per article revision — good. The cap is on *who may trigger the first write and how often*, not on duplicating shared work.

**DoD:** budget config validated at startup; over-budget path returns a clean error and is unit-tested; embeddings appear in the admin usage view; a scripted "summarize 100 distinct articles as one user" run is blocked by the per-user cap.

### 4.2 Per-user resource quotas (FND-188, high)

Add a small, typed quota policy (config + admin-overridable), enforced at creation time:

| Quota | Enforce in | Suggested default (real user) |
|---|---|---|
| Max subscriptions | `crates/db/src/subscriptions.rs:subscribe` + `crates/web/src/feeds.rs:add` + API/GReader/OPML paths | 250 |
| Max saved searches | `crates/db/src/saved_searches.rs:create` | 25 |
| Max categories/folders | `crates/db/src/categories.rs:create` | 100 |
| Max API tokens | `crates/web/src/*` token create | 10 |
| Max integration profiles | `crates/db/src/integrations.rs` | 10 |
| Max public shares | `crates/db/src/public_shares.rs` | 100 |
| Max OPML import size | `crates/web/src/opml.rs` (already bounded ~2000) | keep, make configurable |

Implementation: a `quotas` module in `crates/core` producing an immutable `Quotas` value from config; a helper `assert_within_quota(pool, user_id, kind)` in `crates/db`. Return `AppError::QuotaExceeded` (new variant) → 409 with a clear message. Demo users get a tighter tier (§7.4).

**Invariant note:** quotas are counts of **per-user** rows; they never touch shared content.

**DoD:** each create path rejects past the cap with a tested 409; admin can raise/lower caps; defaults documented in `.env.example` + `docs/CONFIGURATION.md`.

### 4.3 Ingest fan-out cap (FND-189, high)

A single feed, within the 10 MiB body limit, can declare thousands of items; each becomes a shared `articles` row plus extract/embed/summarize jobs. That is an amplification vector and a storage bomb.

**Design**
- Cap items processed per refresh cycle: `FEED_MAX_ITEMS_PER_REFRESH` (e.g. 200) in `crates/feeds/src/parse.rs:461` / `crates/feeds/src/lib.rs:275`. Process newest-first; drop the tail with a counted, logged skip.
- Cap total live articles per feed: `FEED_MAX_ARTICLES` (e.g. 5000); rely on retention (§10.3) to trim.
- These are **shared** bounds (O(1) regardless of subscribers) — set once on the feed, not per user.

**DoD:** a wiremock feed with 10k items creates ≤ cap rows and enqueues ≤ cap jobs; test asserts the cap.

### 4.4 Rate-limit coverage + trustworthy client IP (FND-190, high)

**Coverage:** wrap these currently-unthrottled routes with per-user (and/or per-IP) limiters:
- `POST /feeds` (`crates/web/src/feeds.rs:824`) and `POST /api/v1/feeds`
- `GET /search` (`crates/web/src/search.rs:80`)
- `POST /articles/{id}/summarize` (`crates/web/src/reader.rs:1359`) — tight (e.g. 10/min/user)
- OPML import (`crates/web/src/opml.rs`), media proxy (`crates/web/src/media_proxy.rs`)
- Fever (`crates/web/src/fever.rs:111`) and GReader read/write (`crates/web/src/greader.rs`) — currently only `ClientLogin` is limited.

**Trust:** today the client IP is taken from `X-Forwarded-For` unconditionally (`crates/web/src/extract.rs:19`), so `auth_limiter` is trivially bypassed by spoofing the header. Reuse the trusted-proxy CIDR concept already built for reverse-proxy auth (`REVERSE_PROXY_AUTH_TRUSTED_CIDRS`, hardened in FND-172): only honor forwarded headers from trusted peers; otherwise use the socket peer.

**Multi-instance:** the limiter is in-process (`crates/web/src/ratelimit.rs`). If you run >1 app replica behind a balancer, move counters to Redis (you already have an L2 client) or accept per-replica limits. Document the choice.

**DoD:** each protected route returns 429 past its limit (tested); spoofed `X-Forwarded-For` no longer resets `auth_limiter` behind a trusted proxy; limits are configurable.

### 4.5 Signup anti-abuse — the "Full" tier (FND-191, high)

1. **Email verification** (activate the dormant column). On local registration, create the user **unverified**, send a signed verification link (HMAC token, short TTL), and gate login (or gate feed-add/AI) until `email_verified_at` is set. Column already exists at `crates/db/src/users.rs:10` / `crates/db/migrations/0001_init.sql:27`.
   - **New dependency:** SMTP. Add `SMTP_URL`, `SMTP_FROM`, `EMAIL_VERIFICATION_REQUIRED=true`. A `Mailer` port in `crates/core` with a `lettre`-based adapter (vet version ≥7 days old per `AGENTS.md`). Graceful degradation: if SMTP is down, queue and retry via an agent; never crash signup.
2. **CAPTCHA / Turnstile** on `POST /register` and after N failed logins. Add a pluggable `CaptchaVerifier` (Cloudflare Turnstile or hCaptcha; both are cheap and privacy-friendly) verified server-side via `SafeFetchClient`. Config: `CAPTCHA_PROVIDER`, `CAPTCHA_SITE_KEY`, `CAPTCHA_SECRET`.
3. **Per-account lockout.** Track failed logins per account (not just per IP) with exponential backoff / temporary lock. Complements the existing IP limiter and the generic "invalid email or password" message (`crates/web/src/auth/handlers.rs:162`).
4. **Password hygiene.** Keep Argon2id; raise minimum length to 10–12 and add a top-N breached-password denylist check (embedded small list or k-anonymity range API via `SafeFetchClient`).
5. **OAuth signups** already respect `RegistrationMode` (`crates/web/src/oauth.rs:289`); keep email-verified trust from providers to skip email verification for OAuth users.

**DoD:** unverified users cannot perform privileged actions; register requires a valid CAPTCHA; repeated bad logins lock the account; SMTP outage does not break signup; all covered by handler tests + one e2e.

### 4.6 Content moderation & takedown (FND-192, high)

Because content is shared, you need instance-level moderation, not just per-user controls.

1. **Global article takedown.** Admin action to delete/hide a shared `articles` row everywhere (and its embedding/summary/enclosures via cascade), plus block its URL so it cannot re-ingest. Pair with the blocklist (§5).
2. **Global feed takedown.** Extend admin feed management (`crates/web/src/admin.rs:1075`) with "block" (→ blocklist §5) in addition to the existing "pause".
3. **Abuse reporting.** A "Report" control on articles/feeds → a lightweight `abuse_reports` table (reporter, target, reason, status) → an admin review queue. Rate-limited and CSRF-protected.
4. **Audit trail.** Log moderation actions (who/what/why), mirroring the acquisition-profile audit pattern (`crates/web/src/acquisition.rs`, migration `0040`).

**Invariant note:** takedown operates on **shared** rows (O(1)); it is the deliberate exception to "unsubscribe never deletes shared content" and must be admin-only + audited.

**DoD:** admin can purge a shared article/feed and it stays gone (URL blocked); users can report; reports appear in an admin queue; actions are audited and tested.

---

## 5. Feature: Feed blacklist (with shipped default list)

**Goal:** admins block feeds by **domain** (suffix match) or **exact URL**; blocked feeds cannot be subscribed to (via any path) and existing subscriptions to them are disabled. Ship a curated default blocklist that admins can extend.

**Why domain-first:** URL normalization (`crates/feeds/src/normalize.rs:8`) lowercases scheme/host and strips fragment/default-port but **preserves path case and trailing slash and does not touch query** — so exact-URL blocking is brittle. Domain suffix matching (reuse `host_matches`, `crates/feeds/src/safe_fetch.rs:446`) is robust. Support both.

### 5.1 Data model (migration `0043_feed_blocklist.sql`)
```sql
CREATE TABLE feed_blocklist (
  id          UUID PRIMARY KEY,
  kind        TEXT NOT NULL CHECK (kind IN ('domain','url')),
  pattern     TEXT NOT NULL,            -- normalized host (domain) or normalized URL
  reason      TEXT,
  source      TEXT NOT NULL DEFAULT 'admin' CHECK (source IN ('admin','default')),
  created_by  UUID REFERENCES users(id) ON DELETE SET NULL,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (kind, pattern)
);
-- Plus an audit table mirroring acquisition_profile_audit (0040).
-- Add to feeds: blocked_at TIMESTAMPTZ, blocked_reason TEXT (dedicated, not overloading health_status='disabled').
```
Ship the default list as `source='default'` INSERTs (idempotent `ON CONFLICT DO NOTHING`), updatable by future migrations or a `rustss-cli update-blocklist` command.

### 5.2 Matching logic (in `crates/core`)
A pure `Blocklist` value compiled from rows with `is_blocked(normalized_url) -> Option<Reason>`: exact-URL set + domain suffix match (`host == d || host.ends_with(&format!(".{d}"))`, case-insensitive). Unit-testable; cached (moka) with invalidation on edit.

### 5.3 Enforcement chokepoints
Add one helper `assert_feed_allowed(&state, &normalized_url) -> Result<(), AppError>` and call it **immediately after `validate_url`** at all six add paths (avoids drift):
- `crates/web/src/feeds.rs:849` (UI add) and bookmarklet discovery `:785`
- `crates/web/src/api.rs:141` (REST)
- `crates/web/src/greader.rs:330` (GReader subscribe)
- `crates/web/src/opml.rs:413` (import) **and** `:264` (preview — mark blocked entries)
- `crates/web/src/admin.rs:1260` (catalog create)

Belt-and-suspenders: also guard the fetch agent beside its existing paused check (`crates/agents/src/fetch_feed.rs:103`) so a stale queued job for a now-blocked feed never fetches.

### 5.4 Effect on existing feeds
When an admin adds a blocklist entry: find matching `feeds` rows (`find_by_url` / domain scan), set `blocked_at`/`blocked_reason`, and treat blocked like paused in the 5 refresh-suppression sites (`crates/db/src/feeds.rs:72,114,127`, `agents/src/fetch_feed.rs`, `db/src/jobs.rs:334`). Per-user `user_feeds` rows are personal state — surface a clear "this feed was blocked by an administrator" notice rather than silently deleting them (leave deletion as an explicit admin choice).

### 5.5 Admin UI
New `/admin/feeds/blocklist` page modeled on `crates/web/src/acquisition.rs` + `admin_acquisition.html`: list/add/remove entries, show `source`, show count of affected feeds, and the audit log. Link from `admin_feeds.html`.

### 5.6 Default list content
Seed with well-known malware/phishing/spam/link-farm domains (start from a small vetted set; validate like the catalog audit in `docs/CATALOG_FEED_AUDIT_2026-08-30.md`). Keep it small and defensible; document provenance. Do **not** ship an over-broad list that blocks legitimate publishers.

**DoD:** blocked domain/URL rejected at all six paths (integration-tested); adding an entry disables matching shared feeds; blocked feed never fetches even with a queued job; admin UI works; default list loads idempotently; matching is unit-tested (case, subdomain, trailing dot).

---

## 6. Feature: RSS content cleaning improvements

The pipeline is already strong (triple sanitization, tracking removal, rewrite rules, personal filters, YouTube consent-gating — §3.1). These are the gaps that matter most for a public instance ingesting *diverse, unknown* feeds. Prioritized:

### 6.1 Charset-correct decoding (FND-193, medium — highest cleaning value)
Extraction decodes bytes with `String::from_utf8_lossy` and ignores `Content-Type` charset, BOM, and `<meta charset>` (`crates/extract/src/lib.rs:458`; also `crates/web/src/live_view.rs:326`, `crates/agents/src/youtube_duration.rs:56`). Non-UTF-8 pages (Windows-1252, ISO-8859-1, Shift_JIS, GB2312…) render as `�`. Use `encoding_rs` (already a transitive dep) to detect (Content-Type → BOM → `<meta>` → fallback) and decode. Feeds themselves are safer (`feed-rs` honors XML encoding), but public-page extraction is not.

**DoD:** fixture pages in Windows-1252/Shift_JIS decode correctly; UTF-8 unchanged.

### 6.2 Stored-body size cap (FND-194, medium)
Add a post-sanitize cap on stored/rendered article body length (e.g. `ARTICLE_MAX_STORED_CHARS`, with a visible "content truncated — open original" notice). Prevents a large-but-under-fetch-cap body from bloating the DB and every render (shared cost). Apply in `crates/core/src/html.rs` output or at the DB upsert (`crates/db/src/articles.rs`).

### 6.3 Lazy-load & media normalization gaps (low/medium)
Extend `crates/core/src/html.rs:244` beyond `data-src`/`data-lazy-src`/`data-original`/`data-url` to also recover `data-srcset`/`data-original-set`, `<noscript>`-wrapped `<img>` fallbacks, and (optionally) CSS `background-image`. Improves image fidelity across messy publishers.

### 6.4 Boilerplate list broadening (low)
`boilerplate_label` (`crates/extract/src/readability.rs:328`) is a short English-only token list. Broaden defaults (cookie banners, comment widgets, author bios, paywall gates, more locales) and consider an instance-level configurable boilerplate list (admin-owned, sanitized-after — reuse the rewrite-rule machinery in `crates/core/src/content_rules.rs`).

### 6.5 Near-duplicate detection (P2 — defer)
Dedup is exact `SHA-256(trim(content))` only (`crates/core/src/hash.rs:11`). Cross-posted/syndicated articles with minor differences become distinct rows. A simhash/shingling pass would dedup them but is a larger, riskier change (touches the shared dedup identity and vector space). Defer to post-launch; note it in FINDINGS.

**DoD (6.1–6.4):** each has a targeted unit/fixture test; sanitizer allowlist stays iframe-free (respect FND-108); no regression in the existing sanitizer XSS tests.

---

## 7. Feature: Demo mode (shared guest sandbox, 24h reset)

**Goal (your choice):** real accounts persist; additionally, visitors can try RustSS as **ephemeral demo users** on the *same* instance, and **all demo data is wiped and reseeded every 24h**. The reset must be surgically scoped so it can **never** touch real users or shared seed content.

### 7.1 Model
- Add `users.is_demo BOOLEAN NOT NULL DEFAULT false` (migration `0044`).
- **"Try the demo"** on the landing page provisions an **ephemeral demo user** (no email/password; random handle), flags it `is_demo=true`, seeds it with the featured catalog via the existing `create_with_default_feeds` path (`crates/db/src/users.rs:40`), and mints a session. (Per-visitor accounts avoid the read/seen state collisions a single shared account would cause; a single shared guest account is a simpler fallback if you prefer.)
- Demo sessions are normal sessions (`sessions` cascade on user delete), so the reset logs them out cleanly.

### 7.2 Reset mechanism — an Agent, not an ad-hoc loop (required by `AGENTS.md` §5)
- New `DemoResetAgent` registered in `Registry::with_defaults()` (`crates/agents/src/worker.rs:39`).
- New cron field `demo_reset_schedule` (default `0 4 * * *`) on `WorkerConfig` + a scheduler branch beside the existing cleanup branch (`crates/agents/src/worker.rs:314`): `enqueue_singleton(&ctx.db, "demo_reset")`.
- The agent's `run()` executes, in one transaction:
  1. `DELETE FROM users WHERE is_demo = true` → cascades away subscriptions, read/seen/bookmarks, feedback, patterns, saved searches, sessions, tokens, notifications, categories, preferences, public shares, passkeys, integrations.
  2. Clean non-cascading residue explicitly: `cost_ledger` rows for demo users (FK is `SET NULL`), and any demo-owned `ui_customizations`/`llm_provider_profiles` (owner cascade covers these, but assert none are admin-scoped).
  3. Invalidate caches (per-user namespaces).
- **Preserve:** all real users, shared `feeds`, `feed_catalog_entries` (the seed lives only in migrations and will **not** regenerate if truncated — never truncate it).
- Optionally re-provision one "showcase" demo account with pre-clicked reading state so the demo looks alive immediately after reset, plus the read-only admin demo user (§7.6, `role='admin_readonly'`).

### 7.3 Safety (this is a destructive job on a live multi-user DB)
- Scope **strictly** by `is_demo = true`. Add an integration test that creates a real user + a demo user, runs the reset, and asserts the real user and their data **survive**.
- Gate the whole feature behind `DEMO_MODE=true` (default false); when false, `DemoResetAgent` is a no-op and "Try the demo" is hidden.
- Provide `rustss-cli demo-reset --confirm` (mirror the destructive `reindex --confirm` guard in `bins/cli/src/main.rs`) for manual runs — with the same `is_demo` scoping.
- Never grant a demo user the full `admin` role or let one reach an admin **mutation**. The only admin surface a demo user may reach is the **read-only admin view** (§7.6), which is 403 on every write.

### 7.4 Demo guardrails (protect wallet, shared tables, and other users)
Demo users must be sandboxed:
- **No paid/outbound side effects:** disable LLM summaries (or serve only already-cached summaries), integration webhook delivery, public share creation, API token creation, and provider configuration for `is_demo` users.
- **Tighter quotas & budgets:** a stricter tier of §4.1/§4.2 (e.g. ≤20 subscriptions, ≤3 saved searches, 0 LLM calls).
- **Protect shared space:** restrict demo feed-adds to the **catalog** (or apply the blocklist + SSRF + a low per-demo-session add cap) so demo users can't inject arbitrary feeds that pollute shared tables for real users.
- **Visible banner:** "Demo mode — this account and its data reset daily at 04:00 UTC."

### 7.5 Config & wiring
`DEMO_MODE`, `DEMO_RESET_SCHEDULE`, `DEMO_MAX_SUBSCRIPTIONS`, `DEMO_SESSION_TTL_HOURS`, `DEMO_ADMIN_READONLY` (expose the read-only admin demo, §7.6). Thread config → `WorkerConfig` in both `bins/server/src/main.rs:217` and `bins/worker/src/main.rs:86` (mirror how `retention_age_basis` flows to `ContentCleanupAgent`). Surface `demo_mode` into `AppSettings` for the banner/CTA.

### 7.6 Read-only admin role — the demo "see as admin" experience

**Goal:** let a demo visitor open the admin dashboards and *see how administration works* — without being able to change anything. Implemented as a first-class **read-only admin** capability that is reusable beyond the demo (support staff, auditors, on-call read access).

**RBAC today:** `users.role` is a plain string; only `"admin"` and `"user"` exist in practice (the PRD's `GUEST` in `FR-AUTH-4` is unused). A single `AdminUser` extractor guards **every** `/admin/*` route (`crates/web/src/auth/extractor.rs:157`, checks `user.role == "admin"`, else 403). Crucially, **admin reads are GET handlers and admin mutations are separate POST handlers** (`crates/web/src/lib.rs:170-199`) — so read vs. write is already split by route, which makes a read-only tier clean to enforce.

**Design — split the guard, add one role:**
1. Add role value `"admin_readonly"` (migration `0045`; if `users.role` gains a CHECK constraint, extend it — otherwise just accept the new value in `users::change_role`, `crates/db/src/users.rs`).
2. Add a second extractor `AdminReadUser` that accepts **`admin` OR `admin_readonly`**, and use it on the admin **GET** routes: `/admin`, `/admin/users`, `/admin/feeds`, `/admin/catalog`, `/admin/logs`, `/admin/costs`, `/admin/ai`, `/admin/acquisition`, and the blocklist page (§5).
3. Keep `AdminUser` (admin **only**) on **every mutating** route: `.../users/{id}/role`, `.../invitations`(+revoke), `.../feeds/{id}/status`, `.../maintenance`, `.../catalog` POST, `/admin/ai` create/toggle/delete, `/admin/acquisition` create/assign/delete, and blocklist writes. A read-only admin therefore gets an automatic **403 on every write** — enforcement is **structural**, not cosmetic.
4. Pass `admin_readonly: bool` into the admin templates (they already carry `is_admin`) to render a persistent "Read-only view" banner and hide/disable mutation controls (defense in depth; the server guard is the real gate).
5. `change_role`'s last-admin and self-demotion protections (`FR-ADM-4`) are unchanged, and `admin_readonly` **does not count as an admin** for them.

**Privacy — critical, because the demo shares the instance with real users.** The admin dashboards expose real users' emails (`/admin/users`), real AI **costs** (`/admin/costs`), the real shared-feed inventory, and error logs. A demo read-only admin must **not** see other people's data. Pick one:
- **(a) Redact for read-only admins (recommended default):** mask other users' emails, show costs as aggregates only, suppress raw log detail (logs are already sanitized — FND-131 — but still operational). Keeps data real, low effort.
- **(b) Synthetic admin view:** render the admin pages against seeded/sample data for read-only admins. Cleanest isolation, more work.

**Wiring to the demo:** the daily reset (§7.2) provisions one **showcase demo user** with `is_demo=true, role='admin_readonly'`, reachable via a "Try the admin demo" CTA (or a demo-only toggle that switches a demo session into the read-only admin view). **Never** grant a demo user the full `admin` role.

**DoD:** a read-only admin can `GET` every admin page (200) and receives **403 on every admin `POST`** (tested exhaustively, route-by-route); a normal `user` is still 403 on all admin routes; a read-only admin cannot self-promote or change any role; other users' PII is redacted (or synthetic) in the read-only view; the read-only banner renders and mutation controls are hidden.

**DoD (demo mode overall):** with `DEMO_MODE=true`, "Try demo" provisions a sandboxed account; demo users cannot spend money or create outbound effects; the scheduled reset wipes only demo data; **a test proves a real user survives a reset**; CLI reset works with `--confirm`; the read-only admin demo (§7.6) works; docs updated.

---

## 8. Product, growth & onboarding (public audience)

A public, open-registration site has to **sell itself in the first ten seconds** and let people use the AI features **without you paying for everyone.** Three pieces (all requested):

### 8.1 Per-user "bring your own" AI key — surface what already exists (mostly UX + cost policy)

**Good news: this is already built.** Users can add their own LLM provider + API key today at **`/settings/ai`** (`crates/web/src/ai_settings.rs` — `user_page`/`user_create`/`user_toggle`/`user_delete`, routed at `crates/web/src/lib.rs:265-269`). Keys are **encrypted at rest** (`state.llm_credentials.encrypt_str`, XChaCha20-Poly1305), stored owner-scoped in `llm_provider_profiles`, and the summarizer resolves **user → admin → environment** (`crates/llm/src/lib.rs`). So the capability you asked for exists — what's missing is **discoverability, a Preferences entry point, and a public cost policy.**

**Security constraint to preserve (FND-163):** regular users may only create **OpenAI-compatible profiles pinned to the official `https://api.openai.com` endpoint** (`crates/web/src/ai_settings.rs:157,182`); only admins may set arbitrary base URLs or Ollama. **Keep this on a public instance** — letting anonymous users point the server at an arbitrary base URL is a server-side request-forgery / abuse primitive. (Future enhancement: an *admin-curated allowlist* of provider base URLs — OpenAI, OpenRouter, Anthropic-compatible gateways — instead of a single fixed host.)

**Work to do:**
1. **Surface it in Preferences.** Add an "AI & summaries" entry point from `/settings/preferences` (`crates/web/src/prefs.rs`) that links to (or embeds) the `/settings/ai` manager, with plain copy: *"Add your own OpenAI key to get article summaries. Your key is encrypted and used only for your summaries."*
2. **Cost policy (ties to §4.1).** On the public instance, a user with an **enabled personal key is exempt from the instance LLM budget** — they spend their own quota. This is the scalable model: keep instance-funded summaries off (or on a tiny global budget) and let motivated users bring keys. Users **without** a key get no summaries (or draw from a small shared budget per §4.1). Apply the per-user **rate limit** (job/DB abuse) regardless of who pays.
3. **Onboarding cue.** When a user first hits a summary control with no key configured, show a one-time inline prompt linking to key setup (rather than a dead button — cf. FND-136, which already hides summary controls when no provider exists).
4. **Demo users:** BYO-key is disabled for `is_demo` users (§7.4) — no outbound spend from ephemeral accounts.

**DoD:** Preferences links to the AI key manager with clear copy; a user with an enabled personal key can summarize and is **not** charged against the instance budget; the official-endpoint restriction is intact and tested; demo users cannot add keys.

### 8.2 Marketing landing page — "what we do", with screenshots & features

**Today the landing page is a stub** — `crates/web/templates/home.html` renders one line and two links; the signed-in branch literally says *"The reading experience is under construction."* Not launch-ready for strangers.

**Build a real anonymous landing page** (server-rendered Askama, consistent with the no-SPA/strict-CSP stack — no inline styles/scripts, FND-161):
- **Hero:** the product one-liner (`docs/PRD.md` §1.1: *"Your RSS reader, but it actually understands what you read"*), a primary **"Create free account"** CTA, a secondary **"Try the live demo"** CTA (wired to §7), and "Sign in".
- **Feature highlights** (truthful, all shipped): semantic/vector search, per-user relevance ranking that *learns* from your reading, optional LLM summaries (bring your own key — §8.1), saved semantic searches with alerts & "more like this", three reading modes + 14 themes, OPML import/export, GReader **&** Fever client APIs, PWA install, and self-hostable single binary.
- **Screenshots:** you already have 9 captures in `docs/screenshots/` (`library.png`, `discover.png`, `article.png`, `feeds.png`, `ai-providers.png`, `admin-overview.png`, …) produced by `scripts/capture-screenshots.mjs`. Serve **web-optimized** copies (resize/compress, prefer WebP) as embedded static assets via rust-embed (`crates/web/static/`), and lazy-load them. *Size trade-off:* the raw PNGs total ~2.6 MB — optimize before embedding to protect the <150 MB image target and first paint; extend the capture script to emit optimized landing variants.
- **Footer:** Terms/Privacy/Abuse (§9), source repo, status.
- **Accessibility & responsiveness:** reuse the existing responsive/reduced-motion/contrast work (FND-173/183); the landing page must pass the same no-inline-CSP and six-viewport matrix.
- **SEO/social:** title + meta description, Open Graph / Twitter card image (a screenshot), and `robots`/sitemap so the marketing page is indexable — while authenticated pages stay `noindex`.

**DoD:** anonymous `/` renders a real landing page (hero, features, optimized screenshots, both CTAs); passes strict-CSP + the six-viewport matrix; first paint acceptable; the OG image renders in a link preview.

### 8.3 "RustSS vs Miniflux" comparison on the landing page

Miniflux is the reference point for self-hosted readers and your parity target (`docs/MINIFLUX_PARITY.md`, currently 65/67). A comparison converts visitors who already know Miniflux — **but it must be honest** (overclaiming erodes trust and invites rebuttals). Source it from the parity tracker and keep it dated.

**Suggested framing — *"Everything you expect from Miniflux, plus it understands what you read"*:**

| | RustSS | Miniflux |
|---|:---:|:---:|
| Feeds / OPML / categories, keyboard-driven reading | ✅ | ✅ |
| GReader **&** Fever client APIs (Reeder, NetNewsWire, …) | ✅ | ✅ |
| Full-text extraction, per-feed rewrite rules | ✅ | ✅ |
| Integrations / webhooks to save & notify | ✅ | ✅ (large built-in list) |
| **Semantic (vector) search — find by meaning** | ✅ | ❌ |
| **Personalized ranking that learns from your reading** | ✅ | ❌ |
| **LLM article summaries (bring your own key)** | ✅ | ❌ |
| **Saved semantic searches + "more like this" alerts** | ✅ | ❌ |
| Reading modes (side-panel / inline / standalone) + 14 themes | ✅ | Single clean reader |
| Maturity, install base, minimal footprint | Newer; heavier (Postgres+pgvector, Redis, optional Chromium, embeddings) | ✅ Very mature, tiny footprint |
| Language | Rust | Go |

- **Be fair about trade-offs.** Miniflux is more mature, lighter, and simpler to run; RustSS adds AI/semantic capabilities at a higher resource floor. Saying so builds credibility.
- **Keep it accurate & dated.** Verify each Miniflux cell against current Miniflux docs before publishing (features change), label it *"compared as of &lt;date&gt;"*, and regenerate from `docs/MINIFLUX_PARITY.md`. Keep it a neutral factual comparison (no implied endorsement / trademark misuse).

**DoD:** landing page shows a dated, sourced comparison; every RustSS ✅ maps to a shipped feature; the Miniflux column is verified against current docs; trade-offs are stated, not hidden.

### 8.4 First-run setup wizard (choose layout, theme & typography — with live preview)

New users currently land straight in the reader with defaults (`register_submit`/OAuth → session → redirect `/`, `crates/web/src/auth/handlers.rs:300`). A short, **skippable first-run wizard** lets them pick the essentials up front and *shows off the product's flexibility* — a strong activation lever for a public audience.

**Trigger (run once):** add `users.onboarded_at TIMESTAMPTZ` (migration `0046`). After login, if `onboarded_at IS NULL`, redirect to `/welcome`; "Finish" or "Skip" stamps it. Add a "Re-run setup" link in Preferences so it's repeatable. Demo users see it too (and it resets with them).

**Steps (each a normal form POST; progressively enhanced):**
1. **Reading layout** — side-panel (list + reader **side-by-side**), inline (accordion), or standalone (full page). Maps to `reading_mode` (`crates/web/src/prefs.rs:202`, values `side-panel|standalone|inline`, default `side-panel`).
2. **Theme & colors** — pick from the shipped themes (light / dark / ocean / forest / plum / sunset / system + the rest; 14 total). Maps to `theme` (`prefs.rs:199`).
3. **Typography & density** — font family (sans/serif), reader size, line height, list density. Maps to `font_family` / `reader_font_size` / `reader_line_height` / `article_density`.
4. **(Optional) Topics & AI** — pick starter catalog categories (defaults already auto-subscribe to the featured set via `users::create_with_default_feeds`) and an optional bring-your-own-key AI nudge (§8.1).
5. **Finish** — persist via the existing preferences path (`prefs::save` / a dedicated wizard submit) and set `onboarded_at`; land in the reader.

**Real-time preview — and why it's CSP-safe (the important bit).** RustSS already applies every one of these settings through **data-attributes + classes on `<html>` that key static CSS**: `data-theme`, `data-font-family`, `data-density`, `data-reader-size`, `data-reader-leading`, the article list's `data-reading-mode`, and `sidebar-width-N`/`reader-panel-width-N` (`crates/web/templates/app_base.html:2`; `crates/web/static/app.css` `:root[data-theme="…"]`). So a **live preview needs no inline styles**: the embedded same-origin `app.js` (delegated listeners) just swaps those attributes on a mock reader pane as the user clicks, and the colors/typography/layout update instantly — fully compatible with the strict no-inline CSP (FND-161). This is the clean way to deliver the "real-time color change" you asked for without weakening the CSP.

**No-JS / screenshot fallback (your "or screenshots" option).** Without JavaScript, each step is a plain form with radio options plus a small **representative screenshot/swatch per theme and per layout** (optimized assets reusing the `docs/screenshots/` pipeline, §8.2) so the choice stays visual. This also serves reduced-motion and keyboard users.

**Accessibility/consistency:** reuse the responsive / reduced-motion / contrast + six-viewport matrix (FND-173/183) and the strict-CSP assertions; the wizard is keyboard-navigable and announces step progress.

**DoD:** a brand-new user is routed to `/welcome` exactly once; changing layout/theme/typography updates a **live preview with zero inline styles / no CSP violations**; a no-JS user makes the same choices via form posts with screenshot/swatch previews; selections persist to preferences and `onboarded_at` is set; "Re-run setup" works; covered by a handler test + one e2e (including a no-JS pass).

---

## 9. Legal, privacy & compliance (P0 for a public service) (FND-195)

Operating a public service that stores personal data and fetches third-party content brings obligations a self-hosted box does not:
- **Terms of Service** + **Acceptable Use Policy** (no illegal feeds, no abuse) — static pages (Askama templates) with config URLs; link in the footer and at signup (checkbox).
- **Privacy Policy** — what you store (email, reading state), cookies (functional session cookie), retention, processors (SMTP, CAPTCHA, any AI provider).
- **Abuse / DMCA / takedown contact** — required for hosting third-party content; wire it to the moderation queue (§4.6).
- **GDPR/CCPA:** account deletion already cascades (good); add **self-serve account deletion** in settings and a **full data export** (extend the existing OPML export with reading state/preferences JSON). Email verification supports lawful contact/consent.
- **Age gate / jurisdiction** as appropriate.
- **Cookie/consent banner** — likely minimal (functional cookies only), but confirm for your jurisdiction.

**DoD:** ToS/Privacy/Abuse pages reachable pre-signup; signup records consent; users can export and delete their data; abuse contact routes to the moderation queue.

---

## 10. Operations, scaling & observability for a public instance

### 10.1 Abuse observability (FND-190 adjacent)
Add metrics (you already emit Prometheus via `tracing`/`metrics`): signups/min, feed-adds/min, summarize calls & $/user/day, embedding calls, per-user queue depth, 429 counts. Alert on: global AI budget threshold, queue backlog, error spikes, disk usage. Extend the admin dashboard (`crates/web/src/admin.rs`) with a per-user activity/abuse view.

### 10.2 Scaling
- The Postgres `SKIP LOCKED` queue is fine for target scale but has a throughput ceiling (FND-101) — benchmark against your expected public load.
- In-process rate limiter and L1 cache do not coordinate across replicas; for >1 app instance, use Redis-backed limiting (§4.4) and confirm cache invalidation (already versioned via Pub/Sub, FND-114).
- Chromium is memory-hungry (FND-104); keep it a bounded separate service; consider disabling for demo traffic.

### 10.3 Storage growth
Public ingest grows the shared `articles`/embeddings tables fast. Set a **finite** `ARTICLE_RETENTION_DAYS` for public (currently `0` = forever, `.env.example:163`); bookmarks are always preserved by `ContentCleanupAgent` (`crates/agents/src/content_cleanup.rs`). Monitor DB size; keep the pg_dump backup drill from `docs/RELEASE_EVIDENCE.md` on a schedule.

### 10.4 Deployment hardening
- Terminate TLS (bundled Caddy profiles exist, FND-170); bind the app to localhost behind it.
- Keep `FEEDS_ALLOW_PRIVATE_NETWORK=false`, non-root container, read-only FS where possible.
- Secrets: rotate procedure for `ENCRYPTION_SECRET`/`SESSION_SECRET` (FND-110).

---

## 11. Documentation & truthfulness cleanup (FND-196, low)
- `docs/ARCHITECTURE.md` §rate-limiting says `tower_governor`; code uses `crates/web/src/ratelimit.rs`. Fix.
- `docs/FINDINGS.md` FND-001 is `planned` but conditional GET shipped in M6 — mark resolved.
- After building the above, update `docs/PRD.md` §2.2 (open registration is no longer a strict non-goal), `docs/CONFIGURATION.md`, `.env.example`, and `AGENTS.md` §4 commands.

---

## 12. Phased rollout plan

Each phase is independently shippable and ends with Conventional Commits + the global DoD (`docs/DEVELOPMENT_PLAN.md`). Suggested order optimizes "cheapest thing that most reduces risk first."

| Phase | Theme | Contents | Gate to launch? |
|---|---|---|---|
| **P1** | Cost & compute safety | §4.1 budgets+metering, §4.3 fan-out cap, §4.4 rate-limit coverage & IP trust | **Blocker** |
| **P2** | Account safety | §4.5 email verification + CAPTCHA + lockout, §4.2 quotas | **Blocker** |
| **P3** | Moderation | §4.6 takedown + abuse reports, §5 feed blacklist (+ default list) | **Blocker** |
| **P4** | Legal | §9 ToS/Privacy/Abuse, data export/delete | **Blocker** |
| **P5** | Content cleaning | §6.1 charset, §6.2 size cap, §6.3 lazy-load, §6.4 boilerplate | Recommended |
| **P6** | Demo mode | §7 sandbox + reset agent + guardrails + read-only admin view (§7.6) | Recommended |
| **P7** | Ops & polish | §10 observability/retention/scaling, §11 docs | Recommended |
| **P8** | Product & growth | §8.2 landing page + screenshots, §8.3 Miniflux comparison, §8.1 surface BYO-key AI, §8.4 first-run setup wizard | Recommended |

**Minimum viable public launch = P1–P4.** P5–P8 make it pleasant, sustainable, and actually attractive to sign up for. Note: §8.1's **cost-policy hook** (BYO-key users exempt from the instance budget) should land with **P1**, even though the rest of §8.1's UX is P8.

### Per-phase Definition of Done (in addition to the global DoD)
- P1: over-budget & over-limit paths tested; embeddings metered; fan-out cap tested.
- P2: unverified users gated; CAPTCHA enforced; lockout works; quotas return 409; SMTP outage tolerated.
- P3: blocked feeds rejected at all 6 paths; shared takedown works & is audited; reports queue works.
- P4: legal pages live pre-signup; consent recorded; export/delete work.
- P5: charset fixtures pass; size cap enforced; no sanitizer regressions.
- P6: sandbox enforced; **real user survives reset (tested)**; CLI reset guarded; **read-only admin is 403 on every admin write (tested route-by-route)** and other users' PII is redacted/synthetic in that view.
- P7: dashboards/alerts live; finite retention set; docs reconciled.
- P8: anonymous landing page live (hero, features, optimized screenshots, both CTAs) passing strict-CSP + viewport matrix; dated Miniflux comparison whose ✅ all map to shipped features; BYO-key AI reachable from Preferences and exempt from the instance budget; **first-run wizard routes new users once, live-previews layout/theme/typography with no CSP violations, and has a no-JS screenshot fallback**.

---

## 13. Findings to log (paste into `docs/FINDINGS.md` §B)

| ID | Sev | Status | Finding (short) |
|---|---|---|---|
| FND-187 | critical | open | No AI cost/budget enforcement; embeddings not metered — open reg allows unbounded LLM/compute spend. |
| FND-188 | high | open | No per-user resource quotas (feeds, saved searches, categories, tokens, integrations, shares). |
| FND-189 | high | open | No cap on articles ingested/enqueued per feed refresh — fan-out amplification / storage bomb. |
| FND-190 | high | open | Rate limiting is narrow (auth/api only), in-process only, and the client IP trusts `X-Forwarded-For` unconditionally. |
| FND-191 | high | open | No signup anti-abuse: `email_verified_at` dormant, no CAPTCHA, no per-account lockout, weak password rule. |
| FND-192 | high | open | No global content takedown for shared articles/feeds and no abuse-reporting flow. |
| FND-193 | medium | open | Public-page extraction ignores charset (`from_utf8_lossy`) — non-UTF-8 content corrupts. |
| FND-194 | medium | open | No stored-body size cap after sanitize — oversized shared bodies bloat DB/render. |
| FND-195 | medium | open | No legal surfaces (ToS/Privacy/abuse contact) or full per-user data export. |
| FND-196 | low | open | Doc drift: ARCHITECTURE claims `tower_governor`; FND-001 marked planned though shipped. |

---

## 14. Open questions
1. **Multi-instance?** If you plan >1 app replica, P1/P4 rate limiting must be Redis-backed (§4.4/§10.2). Single instance keeps it simpler.
2. **Email deliverability:** which SMTP provider? (Affects §4.5 setup and Privacy Policy processors.)
3. **CAPTCHA vendor:** Turnstile vs hCaptcha (privacy + cost trade-off).
4. **Demo feed freedom:** restrict demo users to catalog-only feeds, or allow arbitrary (blocklist-guarded) adds? (§7.4 — catalog-only is safer.)
5. **Retention for public:** what finite `ARTICLE_RETENTION_DAYS` is acceptable for your storage budget? (§10.3)
6. **Near-dup dedup (§6.5):** worth the risk post-launch, or leave as exact-hash only?
7. **Read-only admin privacy (§7.6):** for the demo "see as admin" view, redact real users' PII/costs (option a, simpler) or render a fully synthetic admin view (option b, most isolated)? Also: do you want `admin_readonly` available for real staff/support accounts too, or demo-only?
