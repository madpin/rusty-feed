# Rusty Signal — Product Requirements Document

**Product name:** Rusty Signal
**Document status:** Draft
**Version:** 0.2
**Last updated:** 2026-09-07
**Product type:** Self-hosted, API-first RSS curation and feed-generation service
**Companion product:** Rusty Feed

---

## 1. Executive Summary

Rusty Signal is a separate, self-hosted service that turns a large Rusty Feed library into a small number of high-signal, pacing-controlled RSS feeds. It analyzes articles using local semantic embeddings, identifies stories covered by multiple independent sources, applies explicit source and keyword preferences, suppresses semantic repetition, and releases selected articles according to each curated feed's cadence.

Every curated Group has its own standard RSS 2.0 `.xml` endpoint. Users consume those endpoints in existing RSS readers such as Reeder, Feedly, NetNewsWire, or any other standards-compatible client. Developers may also use Rusty Signal as headless middleware for newsletters, dashboards, bots, and knowledge systems.

Rusty Signal does not provide an article-reading experience. Its web interface exists only to connect Rusty Feed, configure Groups, tune curation, inspect explanations and analytics, manage output credentials, and operate the service.

### 1.1 Product one-liner

> Distill your Rusty Feed library into a few high-signal feeds, delivered at the pace you choose.

### 1.2 Problem statement

Power users commonly subscribe to more sources than they can read. Existing feed readers offer folders, unread counts, keyword filters, or opaque recommendation algorithms, but they rarely combine:

- consensus across independent sources;
- explicit source importance;
- strong positive and negative keyword intent;
- temporary interests that expire automatically;
- semantic duplicate suppression based on delivery history;
- controlled output pacing; and
- delivery through ordinary RSS instead of another proprietary reader.

The result is feed fatigue, repeated coverage of the same event, missed important stories, and pressure to abandon open RSS clients for closed recommendation platforms.

### 1.3 Product vision

Rusty Signal should act as a transparent, user-controlled signal layer between Rusty Feed and any downstream RSS consumer. Users should understand why an article was selected, retain direct control over sources and interests, and be able to move to another RSS client without changing their curation setup.

### 1.4 Name rationale

**Rusty Signal** belongs naturally beside Rusty Feed and expresses the product's purpose: extracting signal from feed noise. It is short, works for both a user-facing product and a service binary, and does not imply that it replaces Rusty Feed.

---

## 2. Relationship to Rusty Feed

### 2.1 Product boundary

Rusty Feed is the upstream source of truth for subscribed feeds and canonical article content. Rusty Signal is a separate product with its own API, UI, database, scheduler, workers, curation model, output feeds, and release lifecycle.

| Rusty Feed owns | Rusty Signal owns |
|---|---|
| Account authorization to subscribed feeds | Rusty Signal workspaces and roles |
| Feed discovery, subscription, and normalized source identity | Groups and source membership |
| RSS/Atom/JSON Feed fetching and conditional requests | Per-Group source importance |
| Article parsing, deduplication, and canonical identity | Curation candidates and score explanations |
| Public-page extraction and sanitization | Consensus and semantic-redundancy artifacts |
| Shared article summaries, key points, and topics | Keyword rules and expiration |
| Rusty Feed reader state and personalization | Pacing, generation batches, and delivery history |
| Rusty Feed's active semantic-search embedding space | Rusty Signal's independent curation embedding space |
| Feed and article retention policy | Bounded RSS output snapshots and credentials |

### 2.2 Critical shared-content rule

Rusty Signal must preserve Rusty Feed's defining principle: **content is shared; state is personal**.

When multiple Rusty Signal workspaces independently prove access to the same article on the same Rusty Feed instance:

- the imported article projection is stored once;
- its curation embedding is computed once per article revision and active model;
- reusable semantic-similarity artifacts are computed once;
- workspace authorization is stored separately;
- Group membership, source importance, keywords, scores, pacing, and deliveries remain personal to the workspace or Group; and
- consensus counts and candidate queries include only sources currently authorized to that workspace.

A Group followed by multiple output clients must not cause repeated import, embedding, clustering, or generation work. Serving additional RSS clients should be a cached read of the same committed output snapshot.

### 2.3 Prohibited duplication and coupling

Rusty Signal MUST NOT:

- fetch publisher feeds, article pages, robots files, icons, or media;
- parse source RSS as an alternative to Rusty Feed;
- write to or read directly from the Rusty Feed database;
- request Rusty Feed passwords, browser sessions, or OAuth cookies;
- mutate Rusty Feed read, bookmark, feedback, or learning state as a curation side effect;
- copy full article HTML by default;
- recompute Rusty Feed extraction, sanitization, or summarization;
- expose or depend on Rusty Feed's internal pgvector column; or
- compare, pad, or truncate vectors from different models or dimensions.

If Rusty Feed is unavailable, Rusty Signal continues serving committed output feeds and waits. It never falls back to crawling publishers.

---

## 3. Goals and Non-Goals

### 3.1 Goals

1. **Reduce feed fatigue:** deliver a materially smaller stream while retaining high-value coverage.
2. **Preserve user agency:** make sources, interests, exclusions, pacing, and explanations directly configurable.
3. **Use consensus responsibly:** favor independently repeated coverage without claiming that repetition proves truth.
4. **Prevent repetition:** use semantic history to avoid sending several versions of the same story.
5. **Support temporary intent:** allow short-lived interests and exclusions that automatically become neutral.
6. **Create portable outputs:** publish standard RSS endpoints consumable by common readers and tools.
7. **Integrate cleanly:** use Rusty Feed's tenant-scoped native API and signed events without database coupling.
8. **Compute expensive work once:** share imported content projections and curation embeddings wherever authorization permits.
9. **Operate locally:** provide local embeddings, a low-dependency UI, SQLite persistence, and first-class containers.
10. **Be fully automatable:** expose all management capabilities through a versioned JSON API.
11. **Explain decisions:** retain bounded, versioned reasons for selection and hard rejection.
12. **Degrade safely:** preserve output availability during upstream, model, worker, or webhook failures.

### 3.2 Non-Goals

- An article reader, offline reading store, annotation system, or native mobile app.
- Replacing Rusty Feed's subscriptions, folders, OPML, search, personalization, summaries, or client APIs.
- Independent source crawling, article extraction, summarization, or publisher media proxying.
- Writing generated Groups back into Rusty Feed as subscriptions in v1.
- Training a custom language model.
- Generating new article or cluster summaries in v1.
- Determining factual truth, political balance, or source credibility automatically.
- Sharing private content or aggregate behavior between customers without authorization.
- Paid billing or a public multi-tenant SaaS control plane.
- Guaranteed inclusion when finite cadence has insufficient capacity.
- Arbitrary user-provided code, regular expressions, scoring scripts, or model plugins in v1.

---

## 4. Target Users and Use Cases

### 4.1 Personas

| Persona | Primary need | Typical configuration |
|---|---|---|
| Power reader | Reduce hundreds of daily posts to a manageable stream | 10–20 Groups, hourly/daily pacing, explicit interests |
| Researcher | Track repeated coverage and emerging topics | High consensus weight, temporary project keywords |
| Competitive analyst | Monitor selected organizations and markets | High-priority sources, essential entity phrases |
| Developer | Feed curated data into another system | API-only setup, signed batch webhooks |
| Newsletter operator | Produce a bounded candidate feed for editorial review | Daily cadence, explanations, manual release |
| Small-team administrator | Manage shared curation and access | Workspace roles, audit history, revocable outputs |
| Self-hoster | Keep data local and operations understandable | One Rusty Feed connection, SQLite, local model |

### 4.2 Primary use cases

- “Give me the five most important AI stories each day, not twenty articles about the same release.”
- “Send urgent security stories hourly, but ordinary development news only twice per day.”
- “Favor these three primary sources without excluding broader consensus.”
- “Treat ‘elections’ as essential for the next two weeks, then stop prioritizing it.”
- “Never send cryptocurrency stories to this Group.”
- “Reserve one in ten items for important stories outside my declared interests.”
- “Notify my newsletter workflow whenever a new batch is released.”
- “Use the same curated feed in several RSS clients without multiplying upstream work.”

---

## 5. Scope, Priorities, and Terminology

### 5.1 Priority definitions

- **P0:** required for the first production-ready release.
- **P1:** required for the complete v1 product but may follow the first usable release.
- **P2:** accepted future capability, not required for v1.

### 5.2 Core terminology

- **Workspace:** Rusty Signal tenant containing users, one Rusty Feed connection in v1, Groups, and personal state.
- **Connection:** encrypted credentials and synchronization state for one Rusty Feed account and origin.
- **Source:** an authorized Rusty Feed feed available for Group membership.
- **Group:** one independently configured curation policy and RSS output.
- **Projection:** bounded local copy of curation-relevant Rusty Feed article metadata.
- **Candidate:** an authorized projected article eligible for evaluation by a Group.
- **Event cluster:** semantically similar articles published within a bounded time window.
- **Representative:** the single article selected to represent an event cluster in a Group.
- **Delivery:** immutable record that an article was released to a Group.
- **Generation batch:** one atomic scheduled or manual selection and publication operation.
- **Output credential:** revocable secret embedded in an RSS feed URL.
- **Consensus:** repeated semantically similar coverage by distinct effective publishers, not a truth score.

---

## 6. End-to-End User Flows

### 6.1 First-run setup

1. The first local account becomes Workspace Owner.
2. The Owner enters the Rusty Feed base URL and a revocable API token.
3. Rusty Signal validates origin policy, TLS expectations, API version, token identity, and required access.
4. Rusty Signal imports the authorized source list and starts a bounded article synchronization.
5. Model availability and synchronization progress remain visible; setup does not appear complete while either has an unexplained failure.
6. The Owner creates the first Group or starts from a safe template.
7. The Owner selects sources, sets pacing, reviews defaults, and creates an output URL.
8. A preview shows candidate explanations before the first release.

### 6.2 Create and publish a Group

1. User enters name, description, time zone, and optional slug.
2. User selects one or more Rusty Feed sources.
3. User adjusts source importance or accepts `1.0x` defaults.
4. User selects hourly, daily, interval, delivery-window, and batch limits.
5. User configures keyword rules and serendipity.
6. Rusty Signal validates that the policy is satisfiable and bounded.
7. User previews likely selections and rejection reasons.
8. User enables the Group and creates a revocable RSS credential.
9. Scheduled generation releases items atomically when capacity is available.

### 6.3 Tune interests

1. User opens the Tuning Center for a Group.
2. User adds a term or phrase with weight `+2`, `+1`, `0`, `-1`, or `-2`.
3. User optionally assigns a future start and expiration.
4. Preview immediately re-scores current candidates without publishing them.
5. User reviews examples affected by the rule.
6. The saved rule affects future generations only; previous deliveries remain immutable.

### 6.4 Accept a suggestion

1. Rusty Signal presents a frequently occurring, distinct-source phrase.
2. User inspects representative articles, frequency, trend, and source count.
3. User accepts it with a chosen weight and optional expiration or dismisses it.
4. Accepted and dismissed suggestions are recorded to prevent repetitive prompting.
5. No action modifies Rusty Feed's saved searches or learned patterns.

### 6.5 Manual release

1. User previews the next batch and explanations.
2. User chooses a bounded article count.
3. Rusty Signal queues an idempotent generation job.
4. Normal hard filters and duplicate checks always apply.
5. Cadence bypass, if permitted, requires a distinct action, permission, confirmation, and audit event.
6. The committed batch updates the RSS snapshot and triggers configured outbound webhooks.

### 6.6 Consume an output feed

1. User copies an opaque Group RSS URL into a standard reader.
2. The reader polls with normal conditional requests.
3. Rusty Signal returns the same bounded snapshot to every authorized holder of that output URL.
4. Newly released items use release time as RSS publication time so pacing is visible downstream.
5. User can rotate or revoke a leaked output URL without changing the Group or delivery history.

### 6.7 Recover from missed events or outages

1. Signed Rusty Feed events normally trigger immediate import.
2. Cursor synchronization repairs missing or delayed events.
3. Daily reconciliation repairs source authorization and recent projections.
4. During Rusty Feed downtime, existing RSS snapshots remain available.
5. After recovery, bounded catch-up resumes without exceeding Group cadence unless explicitly configured.

---

## 7. Functional Requirements

### 7.1 Accounts, Workspaces, and Authorization

- **FR-AUTH-1 (P0):** Support local email/password authentication using Argon2id and secure server-side sessions.
- **FR-AUTH-2 (P0):** The first persistent account becomes Workspace Owner through an explicit bootstrap rule.
- **FR-AUTH-3 (P0):** Roles are `OWNER`, `EDITOR`, and `VIEWER`. Owners manage users, connections, secrets, and destructive actions; Editors manage Groups and releases; Viewers inspect configuration and analytics.
- **FR-AUTH-4 (P0):** Every database query and API resource is scoped by `workspace_id`; foreign IDs return `404` without tenant disclosure.
- **FR-AUTH-5 (P0):** Users can create, label, scope, inspect last use, and revoke Rusty Signal API keys.
- **FR-AUTH-6 (P1):** Owners can invite and remove members without exposing Rusty Feed credentials.
- **FR-AUTH-7 (P1):** Owners can require recent authentication for connection changes, output-token rotation, exports, and account deletion.
- **FR-AUTH-8 (P2):** Optional OIDC login may replace or supplement local accounts.

### 7.2 Rusty Feed Connection

- **FR-CONN-1 (P0):** Connect one Rusty Feed account per Workspace using a normalized base URL and revocable API token.
- **FR-CONN-2 (P0):** Validate `/api/v1/version`, `/api/v1/user`, and `/api/v1/feeds` before committing a connection.
- **FR-CONN-3 (P0):** Require HTTPS outside explicitly enabled local development or administrator-approved private-network deployment.
- **FR-CONN-4 (P0):** Encrypt the token at rest, never redisplay it, and mask it from logs, diagnostics, metrics, and exports.
- **FR-CONN-5 (P0):** Show connection identity, Rusty Feed version, last success, synchronization cursor, lag, and safe recent errors.
- **FR-CONN-6 (P0):** Token revocation or authorization failure pauses imports without deleting Groups or committed RSS snapshots.
- **FR-CONN-7 (P0):** Replacing a token requires successful validation and must not create duplicate source or article projections.
- **FR-CONN-8 (P1):** Provide a guided Rusty Feed setup checklist for API token and `feed.new_entries` webhook creation.
- **FR-CONN-9 (P2):** Support multiple Rusty Feed connections per Workspace with explicit source namespaces.

### 7.3 Source and Article Synchronization

- **FR-SYNC-1 (P0):** Bootstrap authorized sources from `GET /api/v1/feeds`.
- **FR-SYNC-2 (P0):** Bootstrap a configurable article lookback, default 30 days, through the tenant-scoped native API.
- **FR-SYNC-3 (P0):** Accept versioned `feed.new_entries` events as wake-up hints, deduplicate them durably, and fetch authoritative article details from Rusty Feed.
- **FR-SYNC-4 (P0):** Run stable cursor synchronization every 15 minutes by default to repair missed events.
- **FR-SYNC-5 (P0):** Run daily source and recent-window reconciliation.
- **FR-SYNC-6 (P0):** Track article revisions. Recompute curation embedding and reusable semantic artifacts at most once for each new revision.
- **FR-SYNC-7 (P0):** Use idempotent upserts keyed by Rusty Feed instance and canonical source/article ID.
- **FR-SYNC-8 (P0):** Removing source authorization excludes undelivered candidates without altering prior deliveries.
- **FR-SYNC-9 (P0):** Honor `429 Retry-After`; use bounded exponential backoff for transient failures.
- **FR-SYNC-10 (P0):** Never make publisher requests when Rusty Feed data is absent or stale.
- **FR-SYNC-11 (P1):** Use bounded batch detail retrieval to avoid one API request per event.
- **FR-SYNC-12 (P1):** Expose a safe “Sync now” action and asynchronous job status.

### 7.4 Groups and Source Importance

- **FR-GRP-1 (P0):** Users can create, read, update, pause, archive, and delete Groups according to role.
- **FR-GRP-2 (P0):** A Group has a name, description, slug, enabled state, time zone, candidate window, history window, pacing policy, serendipity policy, and output retention.
- **FR-GRP-3 (P0):** Users can assign any currently authorized source to one or more Groups.
- **FR-GRP-4 (P0):** Each Group/source membership has an importance multiplier from `0.5x` to `2.0x`, default `1.0x`.
- **FR-GRP-5 (P0):** Importance affects only ranking and representative choice; it never duplicates import, embedding, or upstream work.
- **FR-GRP-6 (P0):** A Group with no active sources cannot generate and displays an actionable state.
- **FR-GRP-7 (P0):** Source removal stops future selection but does not erase delivery explanations.
- **FR-GRP-8 (P1):** Provide reusable Group templates for common hourly, daily, research, and newsletter use cases.
- **FR-GRP-9 (P1):** Support duplicating a Group without copying delivery history or output credentials.

### 7.5 Article Projection and Curation Embeddings

- **FR-EMB-1 (P0):** Store only bounded curation fields: IDs, source, title, canonical URL, author, excerpt, topics, optional Rusty Feed summary, publication time, revision, and import timestamps.
- **FR-EMB-2 (P0):** Full article HTML is excluded by default.
- **FR-EMB-3 (P0):** Embed normalized `title + excerpt + optional shared summary + topics` using a local model.
- **FR-EMB-4 (P0):** Use one active model and vector dimension per Rusty Signal instance.
- **FR-EMB-5 (P0):** Store model, dimension, input revision, embedding status, and generation time with every vector.
- **FR-EMB-6 (P0):** Never compare, pad, truncate, or silently coerce vectors across models or dimensions.
- **FR-EMB-7 (P0):** Compute one vector per Rusty Feed instance/article revision/model and reuse it across independently authorized Workspaces and Groups.
- **FR-EMB-8 (P0):** Model change requires an explicit staged re-embedding operation with progress, validation, and atomic activation.
- **FR-EMB-9 (P0):** Existing output remains available during model load, failure, or backfill.
- **FR-EMB-10 (P1):** Allow offline model pre-seeding and verify required files before activation.

### 7.6 Consensus Engine

- **FR-CONS-1 (P0):** Group semantically similar recent articles into event clusters using the active curation embedding space.
- **FR-CONS-2 (P0):** Default candidate comparison window is 48 hours and default cosine threshold is `0.82`.
- **FR-CONS-3 (P0):** Consensus counts distinct effective publishers rather than entries. Multiple feeds or syndication paths from one publisher count once.
- **FR-CONS-4 (P0):** Consensus contribution is capped to prevent a large syndication network from dominating ranking.
- **FR-CONS-5 (P0):** Consensus for a Workspace includes only cluster members currently authorized through that Workspace's Rusty Feed connection.
- **FR-CONS-6 (P0):** Every cluster retains bounded supporting-source metadata for explanations.
- **FR-CONS-7 (P0):** At most one representative from an event cluster is released to a Group within its history window.
- **FR-CONS-8 (P0):** UI and API label consensus as repeated coverage, never factual verification.
- **FR-CONS-9 (P1):** Allow administrator-bounded threshold and window tuning.

### 7.7 Semantic Anti-Redundancy

- **FR-DEDUP-1 (P0):** A Rusty Feed article ID can be delivered at most once to a Group outside explicit replay.
- **FR-DEDUP-2 (P0):** Compare candidate embeddings with the Group's bounded delivery history.
- **FR-DEDUP-3 (P0):** Default similarity at or above `0.90` in the 30-day history window is a hard semantic duplicate and is skipped.
- **FR-DEDUP-4 (P0):** Similarity from `0.84` to below `0.90` receives a configurable redundancy penalty.
- **FR-DEDUP-5 (P0):** Supporting articles may increase consensus without each becoming a delivery.
- **FR-DEDUP-6 (P0):** Normal retries, repeated events, and repeated API idempotency keys create no duplicate deliveries.
- **FR-DEDUP-7 (P1):** Editors may inspect the prior delivery that caused a semantic rejection.
- **FR-DEDUP-8 (P1):** Explicit replay requires permission, confirmation, and a new audit-linked delivery GUID.

### 7.8 Keyword Weighting

Keyword rules belong to one Group and use a strict five-band model:

| Weight | Name | Required behavior |
|---:|---|---|
| `+2` | Essential | Highest eligible priority tier; selected before lower tiers when finite capacity exists |
| `+1` | Interested | Positive relevance modifier |
| `0` | Neutral | No keyword modifier |
| `-1` | Disinterested | Negative modifier without absolute exclusion |
| `-2` | Muted | Hard exclusion from scheduled, manual, and serendipity selection |

- **FR-KW-1 (P0):** Rules support normalized terms and exact phrases.
- **FR-KW-2 (P0):** Match title, excerpt, topics, and optional Rusty Feed summary.
- **FR-KW-3 (P0):** Matching is Unicode-normalized and case-folded with documented language behavior.
- **FR-KW-4 (P0):** `+2` does not bypass `-2`, source membership, authorization, semantic duplication, safety limits, or finite output capacity.
- **FR-KW-5 (P0):** Strongest negative conflict wins; `-2` is final.
- **FR-KW-6 (P0):** Repeated matches and keyword stuffing have bounded contribution.
- **FR-KW-7 (P0):** Every evaluated candidate records matched rules and their effective contribution.
- **FR-KW-8 (P0):** Arbitrary regular expressions and executable scoring expressions are rejected in v1.
- **FR-KW-9 (P1):** Bulk import/export supports terms, phrases, weights, starts, expirations, and notes.

### 7.9 Time-Decaying Keywords

- **FR-TTL-1 (P0):** Any non-neutral keyword rule may have `starts_at` and `expires_at`.
- **FR-TTL-2 (P0):** Before `starts_at` and at or after `expires_at`, its effective weight is neutral.
- **FR-TTL-3 (P0):** Expiration is checked during scoring, not only by a scheduled job.
- **FR-TTL-4 (P0):** Expired rules remain visible and auditable until deleted.
- **FR-TTL-5 (P0):** Dates are stored in UTC and displayed in the Workspace time zone.
- **FR-TTL-6 (P0):** Users can renew, neutralize, or make an expired rule permanent.
- **FR-TTL-7 (P1):** UI offers safe presets such as 24 hours, 7 days, 14 days, and 30 days.

### 7.10 Scoring and Explainability

- **FR-SCORE-1 (P0):** Scoring is deterministic, versioned, bounded, and covered by golden test fixtures.
- **FR-SCORE-2 (P0):** Hard gates execute before ranking: authorization, Group source membership, active `-2`, prior article delivery, hard semantic duplicate, candidate age, and available cadence capacity.
- **FR-SCORE-3 (P0):** Eligible ranking combines consensus, freshness, metadata completeness, source importance, keyword modifier, and redundancy penalty.
- **FR-SCORE-4 (P0):** `+2` matches form a priority tier above ordinary score ordering; ties use final score, publication time, then stable article ID.
- **FR-SCORE-5 (P0):** Every delivery stores scoring version, component values, matched rules, consensus sources, and bounded reason codes.
- **FR-SCORE-6 (P0):** Every hard rejection stores or derives a bounded reason code without requiring permanent storage of every transient score.
- **FR-SCORE-7 (P0):** Changing score configuration affects future generations only.
- **FR-SCORE-8 (P0):** Preview and generation use the same scoring implementation and snapshot boundary.
- **FR-SCORE-9 (P1):** UI can compare the current scoring version with the previous version on a bounded recent sample.

### 7.11 Serendipity Injector

- **FR-SER-1 (P0):** Serendipity reserves a configurable share of Group output for high-consensus articles with no active `+1` or `+2` match.
- **FR-SER-2 (P0):** Presets are Off `0%`, Low `5%`, Medium `10%`, and High `15%`.
- **FR-SER-3 (P0):** Custom rates are administrator-bounded from `0%` to `25%`.
- **FR-SER-4 (P0):** Allocation is measured over a rolling delivery window so small batches remain possible.
- **FR-SER-5 (P0):** Serendipity never bypasses `-2`, authorization, Group membership, safety rules, or duplicate protection.
- **FR-SER-6 (P0):** Default policy avoids active `-1` matches.
- **FR-SER-7 (P0):** Candidates must meet a minimum consensus and freshness threshold.
- **FR-SER-8 (P0):** If no serendipity candidate is eligible, the slot returns to ordinary ranking instead of delaying the batch.
- **FR-SER-9 (P1):** Analytics report planned versus actual serendipity share and why reserved slots were unavailable.

### 7.12 Pacing and Cadence

- **FR-PACE-1 (P0):** Every Group has independent pacing.
- **FR-PACE-2 (P0):** Support maximum items per rolling hour and per local calendar day.
- **FR-PACE-3 (P0):** Support fixed release intervals and optional allowed delivery windows.
- **FR-PACE-4 (P0):** Use an IANA time zone for calendar-day and delivery-window calculations, including daylight-saving transitions.
- **FR-PACE-5 (P0):** Support maximum batch size, paused mode, and manual-only mode.
- **FR-PACE-6 (P0):** Default is five items per day, no automatic catch-up burst, and at most five items per generation.
- **FR-PACE-7 (P0):** Capacity is calculated transactionally from committed deliveries.
- **FR-PACE-8 (P0):** Missed schedules do not burst unless catch-up is explicitly enabled within an administrator cap.
- **FR-PACE-9 (P0):** Empty candidate sets are successful no-ops and emit no release webhook.
- **FR-PACE-10 (P1):** UI previews the next eligible time and expected capacity.

### 7.13 Scheduled and Manual Generation

- **FR-GEN-1 (P0):** Scheduled generation runs through a durable job queue and Group-scoped lease.
- **FR-GEN-2 (P0):** Selection, capacity accounting, generation-batch creation, and delivery insertion commit atomically.
- **FR-GEN-3 (P0):** The visible RSS snapshot changes only after the generation transaction commits.
- **FR-GEN-4 (P0):** `POST /api/v1/groups/{id}/generate` queues immediate generation with a bounded requested count.
- **FR-GEN-5 (P0):** Manual generation enforces all hard filters and duplicate rules.
- **FR-GEN-6 (P0):** Cadence override requires `generate:override`, explicit request input, audit recording, and UI confirmation.
- **FR-GEN-7 (P0):** API idempotency keys and job retries return the original batch instead of releasing more items.
- **FR-GEN-8 (P0):** Preview performs no delivery, capacity consumption, RSS mutation, or outbound webhook.
- **FR-GEN-9 (P1):** Editors can cancel generation that has not begun; committed batches cannot be silently undone.

### 7.14 RSS Output

- **FR-RSS-1 (P0):** Every Group can expose one or more revocable RSS 2.0 endpoints.
- **FR-RSS-2 (P0):** Output URLs use at least 128 bits of cryptographic entropy and only hashed lookup credentials where possible.
- **FR-RSS-3 (P0):** XML validates, escapes all untrusted values, rejects invalid control characters, and contains no active scripts.
- **FR-RSS-4 (P0):** Each item has a stable non-permalink GUID derived from Group, upstream article, and replay identity.
- **FR-RSS-5 (P0):** Item link uses Rusty Feed's canonical privacy-clean publisher URL.
- **FR-RSS-6 (P0):** Item `pubDate` is Rusty Signal release time so downstream readers observe pacing.
- **FR-RSS-7 (P0):** Original publisher time and source attribution remain visible in item metadata or description.
- **FR-RSS-8 (P0):** Description contains a bounded sanitized excerpt or shared summary, attribution, consensus count, and short explanation. Full article HTML is off by default.
- **FR-RSS-9 (P0):** Feed `ETag`, `Last-Modified`, and `lastBuildDate` change only when the visible snapshot changes; unchanged requests return `304`.
- **FR-RSS-10 (P0):** Output retention is bounded by age and count, default 30 days or 200 items.
- **FR-RSS-11 (P0):** Rotation invalidates the old URL immediately without changing Group delivery history.
- **FR-RSS-12 (P0):** Feed responses disclose no Workspace identity, Rusty Feed credential, read state, bookmark, private note, or raw vector.
- **FR-RSS-13 (P0):** Existing feeds remain available during Rusty Feed and model outages.
- **FR-RSS-14 (P1):** Support optional Atom output with equivalent semantics.

### 7.15 Suggestions

- **FR-SUG-1 (P1):** Generate bounded keyword suggestions from recent Group candidates and consensus clusters.
- **FR-SUG-2 (P1):** Suggestions include phrase, sample article count, distinct-source count, trend, first/last seen, and representative titles.
- **FR-SUG-3 (P1):** Exclude stop words, existing rules, dismissed suggestions, policy-sensitive tokens, and single low-signal-source phrases by default.
- **FR-SUG-4 (P1):** Accepting a suggestion requires explicit weight and optional expiration.
- **FR-SUG-5 (P1):** Accepting or dismissing a suggestion never changes Rusty Feed personalization or saved searches.
- **FR-SUG-6 (P1):** Suggestion generation is asynchronous and cannot block feed publication.

### 7.16 Analytics and Explanations

- **FR-AN-1 (P0):** Report the Group funnel: considered, eligible, muted, disinterested, stale, duplicate, selected, serendipity, and released.
- **FR-AN-2 (P0):** Report pacing use, unused capacity, next eligible release, and recent generation outcomes.
- **FR-AN-3 (P0):** Report consensus distribution and top source/keyword contributions.
- **FR-AN-4 (P0):** Delivery detail shows why the representative won and which sources supported the event.
- **FR-AN-5 (P0):** Analytics are Workspace-scoped and expose no other tenant's article metadata or vectors.
- **FR-AN-6 (P0):** Metrics distinguish “not selected” from hard exclusion.
- **FR-AN-7 (P1):** Preview shows a bounded next-batch simulation with rejection reasons.
- **FR-AN-8 (P1):** Export Group configuration, delivery history, and aggregate analytics in machine-readable form.

### 7.17 API-First Management

- **FR-API-1 (P0):** Every management action available in the UI has an equivalent versioned JSON API operation.
- **FR-API-2 (P0):** Management API lives under `/api/v1` and publishes OpenAPI 3.1 at `/api/v1/openapi.json`.
- **FR-API-3 (P0):** Use revocable API keys with explicit Workspace-scoped permissions.
- **FR-API-4 (P0):** Collections use `{ "data": [...], "meta": {...} }`; single resources use `{ "data": {...} }`.
- **FR-API-5 (P0):** Errors include stable machine code, safe message, request ID, and appropriate HTTP status.
- **FR-API-6 (P0):** Mutations validate bounded typed inputs and support `Idempotency-Key` where duplication is possible.
- **FR-API-7 (P0):** Async operations return `202` with a job resource.
- **FR-API-8 (P0):** Apply per-key rate limits and return `Retry-After` on throttling.
- **FR-API-9 (P0):** Foreign IDs return `404` without cross-tenant disclosure.
- **FR-API-10 (P1):** API keys can be restricted to selected Groups.

### 7.18 Inbound and Outbound Webhooks

- **FR-WH-1 (P0):** Receive Rusty Feed events at a connection-specific endpoint.
- **FR-WH-2 (P0):** Verify exact-body HMAC when configured, validate payload version/size, and compare signatures in constant time.
- **FR-WH-3 (P0):** Deduplicate Rusty Feed delivery IDs durably and acknowledge repeats without reprocessing.
- **FR-WH-4 (P0):** Treat event article fields as hints and retrieve authoritative detail through the authenticated API.
- **FR-WH-5 (P0):** Return quickly after durable enqueue; never embed or generate in the webhook request.
- **FR-WH-6 (P0):** Send optional `group.batch_released` events only after non-empty batches commit.
- **FR-WH-7 (P0):** Outbound payload includes version, event ID, Group ID, batch ID, release time, count, output identifier, and released article IDs/links.
- **FR-WH-8 (P0):** Outbound delivery is at least once with bounded retries, rate limits, pause, test, history, and dead-letter state.
- **FR-WH-9 (P0):** Support exact-body HMAC-SHA256 plus timestamp for outbound events.
- **FR-WH-10 (P0):** Webhook failure never rolls back a committed RSS batch or blocks RSS serving.

### 7.19 Administration and Operations

- **FR-OPS-1 (P0):** Dashboard shows Rusty Feed status, sync lag, model state, queue depth, worker state, storage, and safe recent failures.
- **FR-OPS-2 (P0):** Owners can trigger reconciliation, retry dead jobs, pause generation, and test webhooks.
- **FR-OPS-3 (P0):** Model migration is guarded, observable, cancellable before activation, and auditable.
- **FR-OPS-4 (P0):** Backup and restore include SQLite data, output state, encrypted secrets, and configuration; model files may be recreated separately.
- **FR-OPS-5 (P0):** Export and Workspace deletion affect only Rusty Signal state and never delete Rusty Feed data.
- **FR-OPS-6 (P0):** Audit connection changes, roles, Group policy changes, cadence overrides, output-token changes, API-key changes, exports, and model activation.
- **FR-OPS-7 (P1):** Provide bounded retention controls for projections, event receipts, run records, audits, and deliveries while preserving active output correctness.

---

## 8. Curation Algorithm Contract

### 8.1 Candidate eligibility

At a single read-consistent generation boundary, an article is eligible only when:

1. the Workspace currently has access to its source and article;
2. the source is active in the Group;
3. publication time is within the Group candidate window;
4. a valid active-model embedding exists;
5. no active `-2` rule matches;
6. the exact article has not already been delivered to the Group;
7. no hard semantic duplicate exists in the Group history window; and
8. the Group has cadence capacity, unless an authorized override was explicitly requested.

Candidates awaiting an embedding are deferred rather than ranked with an incompatible fallback. A deployment may add a clearly labeled keyword-only degraded mode after v1, but it must never masquerade as semantic consensus.

### 8.2 Default ranking formula

For ordinary eligible candidates, normalize the following values:

- `consensus` in `[0,1]`, based on capped distinct effective publishers;
- `freshness` in `[0,1]`, using configurable exponential time decay;
- `completeness` in `[0,1]`, based on bounded presence of title, excerpt/summary, author, topics, and canonical link;
- `keyword_modifier` in `[-1,1]`, bounded across matching rules;
- `source_multiplier` in `[0.5,2.0]`;
- `redundancy_penalty` in `[0,0.5]` for near-duplicates below the hard threshold.

The initial versioned score is:

```text
base = 0.45 * consensus + 0.35 * freshness + 0.20 * completeness
rank_score = clamp(base * source_multiplier + 0.30 * keyword_modifier - redundancy_penalty, 0, 2)
```

`+2` essential matches create a priority tier evaluated before ordinary candidates. Within a tier, order by `rank_score DESC`, then original publication time descending, then stable article ID ascending.

The formula is a product default, not an unversioned implementation detail. Any changed weights or transforms require a new `scoring_version`, updated fixtures, migration notes, and retained explanations for previous deliveries.

### 8.3 Consensus score

Default distinct-publisher normalization:

```text
consensus = min(distinct_effective_publishers - 1, 4) / 4
```

This yields no bonus for one publisher and reaches the default cap at five distinct publishers. Publisher identity is derived from Rusty Feed source/site metadata and normalized registrable domain where available. Ambiguous identity is treated conservatively as the same publisher until safely distinguished.

### 8.4 Freshness score

Default freshness uses a 24-hour half-life from original publication time and is clamped to the Group candidate window. Missing publisher time falls back to Rusty Feed import time with an explanation flag.

### 8.5 Keyword contribution

Default per-rule contributions before the aggregate cap are:

- `+2`: priority tier and `+1.0` modifier input;
- `+1`: `+0.4`;
- `0`: `0.0`;
- `-1`: `-0.4`;
- `-2`: hard exclusion.

The aggregate keyword modifier is clamped to `[-1,1]`. Phrase matches count once per rule per article. Exact title matches may receive a bounded multiplier documented by the scoring version.

### 8.6 Representative selection

A cluster contributes all authorized distinct publishers to consensus, but releases at most one representative. Representative selection uses the Group-specific final tier/score. If two members are equivalent, prefer:

1. higher source importance;
2. more complete metadata;
3. canonical publisher URL over a syndication URL;
4. earlier authoritative publication time; and
5. stable article ID as the final deterministic tie-breaker.

### 8.7 Serendipity allocation

Serendipity uses a rolling 100-delivery accounting window or all deliveries when fewer than 100 exist. The allocator attempts to move actual share toward the configured target without exceeding current batch size. A serendipity candidate must:

- pass all ordinary hard gates;
- match no active positive rule;
- avoid `-1` by default;
- meet the configured consensus minimum; and
- rank highest among remaining serendipity-eligible candidates.

No available candidate returns the reservation to ordinary ranking.

---

## 9. Rusty Feed Integration Contract

### 9.1 Current compatible surface

Rusty Signal can bootstrap against the current Rusty Feed native API:

| Method | Rusty Feed path | Purpose |
|---|---|---|
| `GET` | `/api/v1/version` | Compatibility check |
| `GET` | `/api/v1/user` | Verify token identity |
| `GET` | `/api/v1/feeds` | Import authorized sources and health |
| `GET` | `/api/v1/articles` | Paginated recent-library reconciliation |
| `GET` | `/api/v1/articles/{id}` | Import authorized detail, summary, key points, and topics |

The current `feed.new_entries` integration event can provide immediate article IDs. Rusty Signal deduplicates the Rusty Feed `delivery_id` and then fetches the article through `/api/v1/articles/{id}`.

A current full-access API token is compatible. Production setup should use an explicitly provisioned `read` scope or a future narrower `signal:read` scope.

### 9.2 Required Rusty Feed additions for production quality

The complete production integration requires additive, tenant-scoped behavior:

1. **Stable cursor:** `GET /api/v1/articles?cursor=<opaque>&limit=<n>` ordered by a stable `(updated_at, id)` boundary and returning `next_cursor`.
2. **Revision metadata:** list/detail resources expose `created_at`, `updated_at`, and `content_revision` or an equivalent ETag.
3. **Batch detail:** retrieve up to 100 authorized article details by ID.
4. **Complete event envelope:** version, unique delivery/event ID, article ID, feed ID, and creation time.
5. **Reconciliation semantics:** stable handling for retained, deleted, and no-longer-authorized resources without tenant leakage.
6. **Least-privilege token creation:** provision a read-only API token through Rusty Feed's UI/API.

These changes must preserve Rusty Feed's shared article rows and per-user authorization. Raw embeddings are intentionally not part of this contract.

### 9.3 Connection origin safety

Rusty Feed is often deployed on the same private network as Rusty Signal. Therefore:

- administrators define allowed Rusty Feed origins or private CIDRs at deployment level;
- ordinary users cannot turn connection setup into an arbitrary private-network request primitive;
- normalized origin is pinned after successful validation;
- redirects cannot cross origin;
- credentials are never forwarded to another host; and
- DNS/address changes are revalidated according to the deployment allowlist.

### 9.4 Synchronization lifecycle

- Initial sync imports a bounded time window and records a durable high-water mark.
- Events enqueue high-priority article import.
- Cursor sync repairs omissions and updates changed revisions.
- Daily reconciliation refreshes source authorization and checks the recent window.
- Projection retention never removes metadata still required by active RSS snapshots or delivery explanations.
- If one Workspace loses access, shared projection deletion waits until no authorized Workspace, delivery-retention rule, or output snapshot requires it.

---

## 10. Rusty Signal API Contract

### 10.1 Conventions

- Base path: `/api/v1`.
- Authentication: `Authorization: Bearer <rusty-signal-api-key>`.
- Content type: JSON for management resources; XML for feed resources.
- Timestamps: RFC 3339 UTC.
- IDs: UUIDv7.
- Pagination: opaque cursors for unbounded collections.
- Idempotency: `Idempotency-Key` for connection sync, generation, output-token creation/rotation, and webhook tests.
- Request correlation: return `X-Request-Id` and include it in safe error envelopes.
- API rate limits are per key and operation class.

### 10.2 Permissions

Required scopes:

- `connections:read`
- `connections:write`
- `sources:read`
- `groups:read`
- `groups:write`
- `generate`
- `generate:override`
- `analytics:read`
- `outputs:write`
- `webhooks:write`
- `workspace:admin`

### 10.3 Required endpoints

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/api/v1/version` | Service and API version |
| `GET` | `/api/v1/user` | Current identity, Workspace, role, and scopes |
| `POST` | `/api/v1/connections/rusty-feed` | Validate and create the Rusty Feed connection |
| `GET/PATCH` | `/api/v1/connections/rusty-feed/{id}` | Inspect or update connection metadata |
| `POST` | `/api/v1/connections/rusty-feed/{id}/token` | Replace and validate the encrypted token |
| `POST` | `/api/v1/connections/rusty-feed/{id}/sync` | Queue reconciliation |
| `GET` | `/api/v1/sources` | List synchronized authorized sources |
| `GET/POST` | `/api/v1/groups` | List or create Groups |
| `GET/PATCH/DELETE` | `/api/v1/groups/{id}` | Inspect, modify, archive/delete a Group |
| `POST` | `/api/v1/groups/{id}/duplicate` | Copy policy without history/secrets |
| `PUT/DELETE` | `/api/v1/groups/{id}/sources/{feed_id}` | Set/remove source importance |
| `GET/POST` | `/api/v1/groups/{id}/keywords` | List/create rules |
| `PATCH/DELETE` | `/api/v1/groups/{id}/keywords/{keyword_id}` | Update/delete a rule |
| `GET` | `/api/v1/groups/{id}/suggestions` | List current suggestions |
| `POST` | `/api/v1/groups/{id}/suggestions/{suggestion_id}/accept` | Create a rule from a suggestion |
| `POST` | `/api/v1/groups/{id}/suggestions/{suggestion_id}/dismiss` | Dismiss a suggestion |
| `POST` | `/api/v1/groups/{id}/preview` | Simulate a bounded next batch |
| `POST` | `/api/v1/groups/{id}/generate` | Queue immediate generation |
| `GET` | `/api/v1/groups/{id}/deliveries` | Delivery history and explanations |
| `GET` | `/api/v1/groups/{id}/stats` | Funnel, pacing, source, keyword, consensus stats |
| `GET/POST` | `/api/v1/groups/{id}/output-tokens` | List metadata/create output credential |
| `POST` | `/api/v1/groups/{id}/output-tokens/{token_id}/rotate` | Rotate credential |
| `DELETE` | `/api/v1/groups/{id}/output-tokens/{token_id}` | Revoke credential |
| `GET/POST` | `/api/v1/webhooks` | List/create outbound webhooks |
| `PATCH/DELETE` | `/api/v1/webhooks/{id}` | Update/delete webhook |
| `POST` | `/api/v1/webhooks/{id}/test` | Queue signed test delivery |
| `GET` | `/api/v1/webhooks/{id}/deliveries` | Delivery history/dead letters |
| `GET` | `/api/v1/jobs/{id}` | Async job status |
| `GET` | `/api/v1/audit` | Owner-visible audit history |
| `POST` | `/api/v1/exports` | Queue Workspace export |

### 10.4 Generation request

Example request shape:

```json
{
  "count": 5,
  "override_cadence": false,
  "preview_token": null
}
```

A successful queue response returns `202` with job ID. Completion links to one immutable generation batch. The API never returns an additional batch for the same Workspace, operation, and idempotency key.

### 10.5 Output feed path

```text
GET /feeds/{group_slug}/{opaque_token}.xml
```

The slug is descriptive, not an authorization boundary. Only the opaque credential authorizes access. Invalid, expired, or revoked credentials return a non-revealing `404`.

### 10.6 Inbound Rusty Feed hook

```text
POST /hooks/rusty-feed/{connection_public_id}
```

The endpoint validates size before buffering, verifies the configured signature, deduplicates delivery ID, durably enqueues import, and returns without waiting for model inference.

---

## 11. RSS Output Contract

### 11.1 Channel fields

Required channel data:

- Group name as title;
- Group description;
- stable self URL containing the output credential;
- Rusty Signal generator/version;
- `lastBuildDate` from latest committed visible change;
- language when configured; and
- optional image configured by the Group owner.

### 11.2 Item fields

Required item data:

- stable GUID;
- canonical publisher link;
- original title;
- source name and attribution;
- original author when available;
- Rusty Signal release time as `pubDate`;
- original publication time in bounded descriptive metadata;
- sanitized excerpt or existing Rusty Feed summary;
- consensus source count;
- short human-readable reason such as “Essential: passkeys; covered by 4 sources”; and
- categories derived only from safe Rusty Feed topics or explicit Group configuration.

### 11.3 Output stability

- The output snapshot is rendered from committed delivery rows or an atomically replaced cached representation.
- Repeated polling performs no generation and no upstream request.
- Item order is release time descending with stable GUID tie-breaker.
- Tuning changes do not rewrite prior item explanations.
- An article update does not republish it automatically.
- Revoking the final output token does not delete delivery history.

### 11.4 Caching and headers

- Strong or weak ETag is deterministic for the visible snapshot.
- Honor `If-None-Match` and `If-Modified-Since`.
- Send `Referrer-Policy: no-referrer`.
- Use a bounded cache policy suitable for private bearer URLs.
- Do not include cookies in RSS responses.
- Rate limits protect abuse but must permit normal reader polling and conditional requests.

---

## 12. Management UI Requirements

### 12.1 Delivery approach

- Server-rendered Askama HTML.
- HTMX for local progressive updates, vendored and served from Rusty Signal rather than a CDN.
- Critical flows retain standard link/form fallbacks without JavaScript.
- Embedded curated CSS using Rusty Feed-compatible visual tokens where practical.
- No SPA runtime or frontend compilation requirement for production images.

### 12.2 Pages

#### Dashboard

- connection identity and health;
- synchronization progress and lag;
- model and vector-space identity;
- imported, pending, embedded, clustered, eligible, filtered, and delivered counts;
- Group status, next release, and pacing capacity;
- queue/worker health, storage, and safe recent errors.

#### Groups

- Group cards with enabled state, source count, cadence, next release, output state, and recent delivery count;
- create, duplicate, pause, archive, and delete actions;
- source matrix with importance controls;
- pacing, retention, and serendipity controls;
- output-token management and copy action.

#### Tuning Center

- five-band keyword controls;
- phrase entry, effective state, start, expiration, and note;
- suggestion review and one-click acceptance followed by explicit weight confirmation;
- bounded affected-candidate examples;
- preview of current score contributions.

#### Delivery History

- batch and item history;
- representative and supporting sources;
- score components and matched rules;
- duplicate relationship and scoring version;
- filter by source, keyword, reason, batch, and time.

#### Integrations

- Rusty Feed setup/status;
- outbound webhook profiles, signing status, test, pause, retries, and dead letters;
- API-key management;
- RSS output credential management.

#### Administration

- users and roles;
- audit log;
- model migration;
- queues and jobs;
- retention, backup/export, and Workspace deletion;
- effective safe configuration without secret values.

### 12.3 Accessibility

- Meet WCAG 2.2 AA for primary setup, Group, tuning, output, and operations flows.
- Full keyboard operation and visible focus.
- Semantic labels and descriptions for sliders, weights, schedules, and TTLs.
- Status changes announced through appropriate live regions.
- Never rely on color alone for keyword weight or health state.
- Respect reduced motion.
- Charts have equivalent tables or textual summaries.
- Copy-token controls announce success without revealing the token elsewhere.

---

## 13. Technical Architecture Requirements

### 13.1 Runtime and libraries

- Rust stable and Tokio.
- Axum with Tower/Tower HTTP.
- Askama templates and vendored HTMX progressive enhancement.
- `serde`/`serde_json` for API contracts.
- `thiserror` in libraries and `anyhow` only in binaries/job boundaries.
- `tracing` and Prometheus-compatible `metrics`.
- Typed layered configuration through environment and optional file.
- UUIDv7 IDs and UTC timestamps.

Dependencies must be pinned to vetted versions. No runtime CDN, floating dependency, or network-loaded executable asset is allowed.

### 13.2 Persistence

SQLite in WAL mode is the default self-hosted database. Use a maintained pinned vector extension such as `sqlite-vec`, selected after implementation-time validation.

Requirements:

- foreign keys enabled on every connection;
- versioned safe migrations;
- one authoritative writer process or a coordinated writer boundary;
- bounded busy timeout and actionable lock-contention errors;
- transactional generation and publication;
- durable job, lease, event-dedup, delivery, and audit state;
- uniqueness constraints enforcing projection, embedding, receipt, idempotency, and Group-delivery invariants;
- documented online backup/restore; and
- integrity check during administrative validation.

A future PostgreSQL adapter may support horizontal writes. Rusty Signal must never share Rusty Feed's schema, migration stream, or database credentials.

### 13.3 Local model

- Default model: BGE-small-en-v1.5 or a similarly small multilingual model selected before implementation freeze.
- Runtime: `fastembed`/`ort` or an equivalently maintained Rust-native adapter.
- Persistent model cache with explicit online fetch and offline pre-seed commands.
- Bounded model load and inference concurrency.
- Model and dimension guard at startup and write time.
- Staged backfill into a separate model space followed by validation and atomic activation.
- No hash embedding in production except an explicitly labeled development/test provider.

### 13.4 Background runtime

All recurring and event-driven work uses a single durable, observable framework. Required job kinds:

- source synchronization;
- article detail import;
- article embedding;
- similarity/cluster maintenance;
- scheduled Group generation;
- keyword-expiration maintenance;
- suggestion generation;
- outbound webhook delivery;
- retention cleanup;
- model backfill; and
- export.

The runtime provides:

- at-least-once execution;
- idempotent handlers;
- bounded payloads;
- scheduled UTC jobs with Group time-zone calculations in domain logic;
- lane-isolated concurrency so model backfills cannot starve RSS/API work;
- leases and stale-lock recovery;
- bounded exponential retry;
- dead-letter state;
- heartbeats and queue depth;
- sanitized run records; and
- graceful shutdown.

### 13.5 Deployment topology

Default Compose topology:

```text
RSS clients ───────────────┐
Browsers/API clients ──────┼──> rusty-signal (Axum + workers) ──> SQLite + vector extension
Rusty Feed webhook ────────┘                │
                                            ├──> local model volume
                                            ├──> Rusty Feed native API
                                            └──> configured outbound webhooks
```

Rusty Feed and Rusty Signal remain independently versioned and restartable. They may share a private container network but not a database.

### 13.6 Graceful degradation

| Failure | Required behavior |
|---|---|
| Rusty Feed unavailable | Serve existing RSS/UI; pause import and generation requiring new candidates |
| Local model unavailable | Serve existing RSS/UI; expose degraded state; defer semantic processing |
| Embedding backlog | Preserve responsiveness through queue lanes and bounded generation deferral |
| Outbound webhook failure | Retry/dead-letter without affecting committed batches |
| Scheduler interruption | Resume from durable state without unbounded catch-up |
| SQLite busy | Retry within bounded timeout; return safe actionable error; never partially publish |
| Corrupt candidate | Quarantine candidate and continue unrelated Groups |
| Invalid output token | Non-revealing `404` |

---

## 14. Logical Data Model

### 14.1 Identity and personal state

```text
workspaces
users
workspace_memberships
sessions
api_keys
rusty_feed_connections
workspace_source_access
workspace_article_access
audit_events
```

### 14.2 Shared imported content

```text
upstream_instances
source_projections
article_projections
article_embeddings
article_similarity_edges or event_clusters
cluster_members
```

Shared rows are keyed by upstream instance plus canonical Rusty Feed entity ID. Access rows are required before any Workspace can query them.

### 14.3 Group state

```text
groups
group_sources
keyword_rules
keyword_suggestions
suggestion_decisions
group_scoring_versions
generation_batches
deliveries
delivery_explanations
output_tokens
```

### 14.4 Operations

```text
inbound_event_receipts
jobs
job_runs
worker_heartbeats
leases
webhook_profiles
webhook_deliveries
idempotency_records
exports
```

### 14.5 Required uniqueness and isolation

- one connection per Workspace in v1;
- one shared source projection per upstream instance/source ID;
- one shared article projection per upstream instance/article ID;
- one embedding per article projection/model/revision;
- one Workspace access row per connection/source or article;
- one Group/source membership;
- one normalized active keyword rule identity per Group unless date windows do not overlap;
- one normal delivery per Group/article;
- one inbound receipt per connection/delivery ID;
- one idempotency result per Workspace/key/operation; and
- one token hash per credential class.

Foreign keys and repository APIs must make it impossible to use a shared projection without a current Workspace access join.

---

## 15. Security and Privacy

### 15.1 Secrets

- Encrypt Rusty Feed tokens and webhook credentials with XChaCha20-Poly1305 or an equivalent misuse-resistant AEAD.
- Derive domain-separated keys from one deployment secret with versioned key identifiers.
- Store hashes rather than recoverable plaintext for Rusty Signal API keys and output credentials wherever possible.
- Show a newly generated bearer value once.
- Support rotation without plaintext redisplay.
- Never include secrets in URLs except the intentionally bearer-style RSS output credential.

### 15.2 Web security

- Secure, HTTP-only, SameSite session cookies.
- CSRF protection on every cookie-authenticated mutation.
- Strict CSP with no inline script requirement.
- `Referrer-Policy: no-referrer` on output feeds and strict application policy on UI/API responses.
- Standard clickjacking, MIME-sniffing, and transport headers.
- Request body, JSON field, XML output, pagination, batch, and time-window limits.
- Password and authentication rate limiting.
- Per-IP and per-credential limits on hooks, API, generation, sync, and RSS endpoints.

### 15.3 Outbound request safety

- Rusty Feed origins are deployment-allowlisted and pinned.
- Private-network access requires explicit administrator configuration.
- Generic outbound webhook URLs receive SSRF checks, DNS/private-range validation, safe redirects, size/time limits, and per-domain throttling.
- No credentials cross origins or redirects.
- Logs use destination host and safe status only, not secret headers or response bodies.

### 15.4 Content safety

- Treat all Rusty Feed strings as untrusted even though upstream sanitizes article HTML.
- RSS XML escapes every field and rejects forbidden control characters.
- UI renders excerpts and explanations as escaped text or re-sanitized bounded HTML.
- No raw vector, full webhook body, article content, or user keyword list appears in metrics.

### 15.5 Privacy and tenant isolation

- Every personal read/write is Workspace-scoped.
- Shared article projections are inaccessible without independently proven Workspace authorization.
- Consensus is calculated from the authorized subset.
- Analytics are personal aggregates.
- Workspace export includes only that Workspace's access, configuration, and personal state.
- Workspace deletion removes its access and personal state; shared projections remain only while another authorization or retention dependency exists.

---

## 16. Non-Functional Requirements

### 16.1 Performance targets

Targets assume one installation with up to 10 Workspaces, 500 sources per Workspace, 250,000 retained shared article projections, 100 Groups, and a 2-vCPU/2-GB host.

| Metric | Target |
|---|---:|
| RSS unchanged conditional response, p95 | `< 50 ms` |
| RSS changed snapshot response, p95 | `< 100 ms` |
| Non-analytics management API, p95 | `< 100 ms` |
| Inbound webhook acknowledgement, p95 | `< 100 ms` |
| Warm candidate generation for one Group, p95 | `< 2 s` |
| Event to eligible candidate with webhook, p95 | `< 2 min` |
| Missed-event convergence at defaults | `< 20 min` |
| Duplicate deliveries from retries | `0` |
| Publisher feed/article requests by Rusty Signal | `0` |
| Idle application RSS excluding loaded model | `< 100 MB` target |
| Loaded application RSS including local model | `< 500 MB` target |

Embedding throughput is hardware-dependent and must be observable. Backpressure must protect API and RSS serving.

### 16.2 Scalability

- Correct single-node behavior through the target dataset.
- O(1) projection and embedding work per upstream instance/article revision/model, not per Workspace or Group.
- O(1) RSS generation work per committed Group snapshot, not per downstream client.
- Group-specific scoring may scale with candidate Groups but remains bounded by candidate window and source membership.
- SQLite supports one authoritative write topology; horizontal write scaling is explicitly deferred.

### 16.3 Reliability

- Durable at-least-once jobs with idempotent effects.
- Atomic generation and output visibility.
- Restart-safe event receipts and synchronization cursor.
- Existing outputs survive upstream/model/webhook failures.
- Backups produce a consistent restorable database.
- Safe migrations fail before serving writes when incompatible.

### 16.4 Portability

- Linux `amd64` and `arm64` release images.
- Non-root container.
- Persistent data and model volumes.
- Read-only root filesystem where practical.
- No mandatory Node runtime in production.
- Local development on macOS and Linux.

### 16.5 Accessibility and localization

- WCAG 2.2 AA primary-flow target.
- UTF-8 throughout.
- IANA time-zone support.
- Locale-neutral storage and RFC 3339 API timestamps.
- English UI in initial release; localization-ready message structure is P1.

### 16.6 Maintainability

- Modular Rust libraries with narrow domain interfaces.
- No direct SQL outside the persistence adapter.
- Runtime SQLite queries validated by integration tests.
- Public APIs documented and versioned.
- Deterministic algorithm fixtures and migration tests.
- `cargo fmt`, clippy with warnings denied, and full tests gate releases.

---

## 17. Configuration and Operations

### 17.1 Configuration groups

Required typed configuration includes:

- server bind/base URL and trusted proxy policy;
- session, encryption, and cookie secrets;
- SQLite path, WAL/busy timeout, migration policy, and backup location;
- model provider, name, dimension, cache path, batch size, and concurrency;
- Rusty Feed origin allowlist/private CIDRs and synchronization intervals;
- worker lane concurrency, lease, retry, and scheduler settings;
- default Group windows, thresholds, cadence bounds, and retention;
- API/hook/RSS rate limits;
- webhook destination policy;
- tracing, metrics, and log level; and
- retention for projections, receipts, jobs, audit, and exports.

Every value has a documented default, allowed range, startup validation, and secret-safe resolved summary.

### 17.2 Backup and restore

- Use SQLite online backup or an equivalent consistency-safe mechanism.
- Backups include schema, personal state, output state, encrypted connection credentials, and audit data.
- Model files are separately cacheable and may be re-fetched or restored.
- Restore validates schema version, database integrity, encryption-key compatibility, and model identity before enabling generation.
- Documentation includes rollback and recovery when the encryption secret is unavailable.

### 17.3 Retention

Defaults:

- candidate projections: 90 days unless required by delivery/output;
- visible RSS items: 30 days or 200 items per Group;
- inbound event receipts: 30 days after terminal processing;
- successful job runs: 30 days;
- failed/dead job runs: 90 days;
- audit events: one year;
- delivery explanations: retained with delivery history until Workspace policy removes them.

Cleanup is idempotent and must preserve active output snapshots, current authorization, and replay/idempotency correctness.

---

## 18. Observability

### 18.1 Health

- `/health/live`: process can serve requests.
- `/health/ready`: database migrated and writable, required secrets valid, and server ready for core operations.
- Optional dependency states for Rusty Feed, model, and outbound delivery appear separately and do not make committed RSS unavailable.

### 18.2 Metrics

Prometheus-compatible metrics include:

- HTTP count/latency/status by route template;
- RSS `200`/`304`/invalid-token/rate-limit counts;
- Rusty Feed sync lag, pages, articles, updates, errors, and throttling;
- inbound webhook valid/invalid/duplicate counts;
- job queue depth, wait, run, retry, dead-letter, and worker heartbeat;
- model load state, embedding queue, throughput, duration, and failure;
- cluster size and authorized consensus distribution;
- candidate, hard-rejection, near-duplicate, essential, ordinary, serendipity, and delivery counts;
- pacing capacity and unused slots;
- outbound webhook success/retry/dead-letter; and
- database size, projection count, and output snapshot count.

Metrics use bounded labels and never include Workspace email, Group name, keyword, article title/URL, token, or raw ID where cardinality is unbounded.

### 18.3 Logs and traces

- Structured `tracing` output.
- Request, event, job, batch, and delivery correlation IDs.
- Safe error taxonomy.
- No secret values, webhook bodies, full article content, or keyword sets.
- Destination errors record bounded host/status/category rather than sensitive response text.

---

## 19. Testing and Acceptance

### 19.1 Unit tests

- keyword normalization, phrase matching, conflict precedence, and TTL boundaries;
- consensus normalization and publisher deduplication;
- score formula, priority tiers, tie-breaking, and version fixtures;
- similarity thresholds and history windows;
- serendipity rolling allocation;
- cadence across hour/day/time-zone/DST boundaries;
- RSS escaping, stable GUID, ETag, and retention;
- token hashing, encryption, signature verification, and redaction.

### 19.2 Persistence integration tests

- one shared projection/vector for N independently authorized Workspaces;
- no access to a shared projection without Workspace access row;
- Group/source and keyword tenant isolation;
- duplicate inbound event creates one import job;
- job retry creates one embedding and one delivery;
- concurrent generation respects cadence and creates one batch;
- idempotency key returns the original result;
- output-token rotation immediately invalidates prior lookup;
- cleanup preserves visible output and required explanation state;
- backup/restore preserves committed feed behavior.

### 19.3 Rusty Feed contract tests

- current version/user/feed/article envelopes;
- unauthorized or foreign articles return non-disclosing failure;
- offset bootstrap and cursor synchronization;
- article revision updates;
- batch detail bounds;
- duplicate/out-of-order/missing webhook events;
- `429 Retry-After`, transient `5xx`, timeout, and token revocation;
- Rusty Feed outage with continued RSS serving;
- no publisher fallback requests.

### 19.4 HTTP and browser tests

- first-run setup;
- Group creation and source weighting;
- all keyword bands and TTL editing;
- serendipity and pacing controls;
- preview versus release behavior;
- cadence override permission/confirmation;
- output create/copy/rotate/revoke;
- API-key scopes and cross-tenant rejection;
- webhook test/history/dead-letter;
- no-JavaScript critical flows;
- keyboard, focus, live status, reduced motion, and chart alternatives.

### 19.5 RSS client compatibility

Automated fixtures and replay tests must cover at least three common RSS clients or parsers. Acceptance includes:

- valid RSS 2.0;
- stable item identity;
- observed release pacing;
- correct conditional GET;
- descriptions safe and readable;
- tokenized URL accepted;
- rotation/revocation behavior; and
- bounded old-item removal without republishing.

### 19.6 Fault injection

- process restart during import, embedding, generation, snapshot visibility, and webhook delivery;
- stale lease recovery;
- model failure during backfill;
- SQLite busy/low-disk conditions;
- Rusty Feed partial and total outage;
- malformed upstream JSON and inbound webhook payload;
- invalid XML characters in article metadata;
- outbound destination timeout and repeated failure.

---

## 20. Product Success Metrics

1. **Integration completeness:** every authorized Rusty Feed article in the configured window reaches a projection or visible terminal error.
2. **No duplicate acquisition:** Rusty Signal makes zero publisher feed/article-page requests.
3. **Shared compute:** each upstream instance/article revision is projected and embedded once per active model, independent of Workspace and Group count.
4. **Pacing correctness:** normal generation never exceeds configured hourly/day capacity.
5. **Mute correctness:** no article matching an active `-2` rule is released through normal, manual, or serendipity generation.
6. **Delivery idempotency:** event, job, and API retries create zero duplicate normal deliveries.
7. **Redundancy reduction:** at least 95% of labeled high-similarity same-event candidates produce no more than one Group delivery within the history window.
8. **Explainability:** every delivery and hard rejection has a scoring version and bounded reason set.
9. **Serendipity correctness:** actual long-window allocation converges to configured rate when sufficient eligible candidates exist.
10. **Interoperability:** generated RSS passes validation and client replay acceptance.
11. **Output availability:** committed XML remains available during upstream, model, and outbound webhook outages.
12. **User outcome:** beta users report a meaningful reduction in daily article volume without unacceptable loss of essential stories.

---

## 21. Delivery Plan

### Phase 0 — Foundation and Rusty Feed Contract

- workspace/auth/API-key foundation;
- encrypted Rusty Feed connection;
- current API bootstrap and signed event receipt;
- stable cursor/revision/batch Rusty Feed additions;
- shared projection/access schema;
- durable jobs, health, metrics, and tenant-isolation tests.

**Exit:** authorized recent articles converge without publisher requests or cross-tenant access.

### Phase 1 — Semantic Curation Core

- local single-space model;
- shared embeddings and model guard;
- consensus artifacts and authorized counts;
- source importance;
- anti-redundancy;
- deterministic scoring/explanations;
- Group CRUD and preview.

**Exit:** deterministic fixture corpus produces explainable representatives with no repeated per-Group embeddings.

### Phase 2 — Pacing and RSS Product

- cadence/capacity engine;
- durable scheduled/manual generation;
- transactionally committed batches;
- RSS 2.0 output, conditional GET, retention, and revocable credentials;
- outage serving and client compatibility tests.

**Exit:** users can consume paced, stable curated feeds in three tested clients.

### Phase 3 — Preference Controls

- five-band keywords;
- TTL lifecycle;
- conflict behavior;
- serendipity allocator;
- suggestions;
- impact/funnel analytics.

**Exit:** every original curation control is exposed through API and accessible UI with deterministic tests.

### Phase 4 — Operations and Complete v1

- roles/invitations;
- outbound webhooks and dead letters;
- audit, export, backup/restore, retention;
- model migration workflow;
- complete API/OpenAPI;
- performance, fault injection, security, and accessibility acceptance.

**Exit:** all P0 and P1 requirements meet the Definition of Done.

---

## 22. Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Current Rusty Feed API uses offset pagination | Missed/inefficient updates | Add stable cursor, revisions, and batch detail before production sign-off |
| Duplicate local embedding work across services | Extra compute | Rusty Signal embeds only bounded curation text once per upstream article revision; never per Group; no publisher/enrichment duplication |
| Consensus mistaken for truth | Misleading product behavior | Label as repeated coverage, show supporting sources, never call it factual confidence |
| Syndication inflates consensus | Poor ranking | Count normalized effective publishers and cap contribution |
| SQLite write contention | Delayed jobs | One writer boundary, WAL, bounded lanes, busy timeout, explicit single-node target |
| Local model memory/latency | Self-hosting footprint | Small pinned model, lazy bounded inference, persistent cache, isolated backfill lane |
| Tokenized RSS URL leaks | Unauthorized feed access | High-entropy token, one-time display, labels, last use, immediate rotation/revocation |
| Webhooks are missed or duplicated | Sync gaps or repeated work | Durable event dedup plus cursor and daily reconciliation |
| Essential terms overwhelm cadence | Lower-priority starvation | `+2` is a priority tier within finite capacity; analytics expose backlog and unused/insufficient capacity |
| Negative/positive rule conflicts | Surprising output | Strongest negative wins and preview exposes effective decision |
| Temporary rules remain active | Persistent skew | Enforce expiration at score time and through maintenance |
| Shared projections leak tenant content | Privacy incident | Separate access relations, mandatory authorization joins, cross-tenant integration tests |
| Output clients poll aggressively | Resource pressure | Cached snapshots, conditional GET, output-key rate limits, no generation on reads |
| Model change corrupts comparisons | Wrong curation | One active dimension, staged re-embed, atomic activation, no pad/truncate |

---

## 23. Definition of Done

A requirement is complete only when:

- API and UI behavior are implemented where applicable;
- Workspace authorization and shared-content invariants are enforced;
- inputs, limits, inheritance, and error behavior are documented;
- secrets are encrypted, masked, and absent from logs;
- migrations and rollback/recovery behavior are tested;
- deterministic unit and persistence integration tests pass;
- accessibility is verified for user-facing flows;
- metrics and safe operational diagnostics exist;
- Rusty Feed contract and outage behavior are tested;
- `cargo fmt --all --check` passes;
- `cargo clippy --all-targets --all-features -- -D warnings` passes;
- `cargo test` passes; and
- browser/RSS compatibility suites pass for affected behavior.

---

## 24. Confirmed Product Decisions

- Product name is **Rusty Signal**.
- Rusty Signal is a separate service, not a Rusty Feed crate or direct database extension.
- Rusty Feed remains authoritative for source acquisition and article enrichment.
- Rusty Signal computes an independent local curation embedding because raw Rusty Feed vectors are not an API contract.
- Imported projections and embeddings are shared by upstream instance/article revision; access and delivery state remain personal.
- One active curation model/dimension is allowed per Rusty Signal instance.
- SQLite plus a maintained vector extension is the v1 persistence target.
- Axum, Askama, and locally vendored HTMX provide the API and slim management UI.
- RSS 2.0 is the required output format; Atom is P1.
- RSS `pubDate` uses Rusty Signal release time to preserve pacing semantics.
- `+2` is the highest eligible priority, not permission to bypass mutes, duplicates, authorization, safety, or finite capacity.
- `-2` applies to scheduled, manual, and serendipity generation.
- Serendipity expands neutral coverage but never bypasses explicit negative or safety controls.
- Previously released items and explanations are immutable; tuning affects future releases.
- Current Rusty Feed APIs support a functional bootstrap, while cursor/revision/batch additions are required for production quality.
