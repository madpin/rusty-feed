# Rusty Feed — Glossary

One word per concept. Use these words in every screen, notification, email,
error message, and user-facing document. Engineers keep the internal names in
the right-hand column (code, tables, routes, log lines) so that nothing has to
be migrated; they must not leak into the interface.

`crates/web/tests/vocabulary.rs` enforces the "internal terms" list at the end
of this page against all templates. Add a term there when you add one here.

## Product words

| Concept | Say | Never say (in the UI) | Internal name |
|---|---|---|---|
| The personal daily summary of stories | **Briefing** (nav item, page title, email subject, settings) | digest, daily digest, news digest | `DailyDigestAgent`, `daily_digest`/`digest_email` preferences, `digests` table, `db::digests`; route `/briefing` (`/digests` and `/digests/refresh` are permanent redirects) |
| One event covered by two or more of your feeds | **Story** | cluster, group, coverage | `story_clusters`, `ClusterStoriesAgent`; created only at similarity ≥ 0.82 (`STORY_CLUSTER_MIN_SIMILARITY`) |
| A site you subscribe to (RSS, Atom, JSON Feed) | **Feed** ("subscribe to a feed", "your feeds") | source, publication, subscription (as a noun for the thing you read) | `feeds`, `user_feeds` |
| Personal, nestable container for feeds | **Folder** | category | `user_categories` table (Google Reader calls it a category/label; that stays in the GReader API only) |
| Flexible, many-per-feed label you type | **Tag** | label | `user_feed_categories` |
| Grouping of feeds on **Discover** | **Topic** | category | `feed_catalog_entries.category` column and `?category=` URL parameter |
| The curated, admin-edited list shown on Discover | **Discover catalog** (admin) / **Discover** (reader) | feed magazine, magazine, starter sources | `feed_catalog_entries`, `/catalog`, `/admin/catalog` |
| A persistent query that matches new articles | **Saved search** | smart feed, watch | `saved_searches` |
| Told about a strong saved-search match | **Alert** (nav item **Alerts**) | notification (for saved-search matches) | `user_notifications`, `/notifications` |
| An article you keep | **Save** / **Saved** (bookmark) | star, favorite | `bookmarks`; routes `/articles/{id}/bookmark` |
| Finished with an article | **Read** / **Unread** (action: *Mark read*) | done, archived | `read_articles` |
| A card was on screen, or you opened the article | **Seen** / **Unseen** (action: *Mark unseen*) | glance, glanced, viewed | `seen_articles`, `data-skip-glance`, `/articles/{id}/seen` |
| Where you read the articles of your feeds (`/articles`) | **Library** | home, inbox, reader (as a page name) | nav key `library` |
| The AI-written short overview of one article | **AI summary** (kicker: **TL;DR**) | digest, “At a glance” | `article_summaries` |
| One cohesive AI-written article from a Feed or Folder over the last day or week | **Roundup** (nav **Roundups**; **Summarize: Day / Week** control; **Schedule** for repeats) | digest, newsletter, briefing | `roundups`, `roundup_schedules`, `roundup_runs`; `RoundupAgent`, `RoundupScheduleAgent`; `/roundups` and `/roundups/{feed|folder}/{id}` |
| Numeric fingerprint that lets search match by meaning | **search index** (explain “embeddings” only on admin pages) | embedding, vector | `article_embeddings`, `embed` crate |
| The setup of how to fetch a feed that needs special access | **Feed access profile** (admin) | acquisition profile/policy | `acquisition` module, `/admin/acquisition` |
| Processes that run background work | **Background work**, **task**, **worker** (admin pages only, always explained) | job, agent, lane, heartbeat, outbox, dead letter | `job_queue`, `Agent`, queue lanes `interactive`/`background`, `job_worker_heartbeats`, outbox, `dead` status |
| A task's priority class | **Priority** (priority / background) | lane | queue lane |
| A task that used up its retries | **Failed task** | dead-letter job | `dead` job status |
| Worker has not been heard from | **has not checked in** | missed heartbeat | heartbeat |

## Reading states in one picture

```
            seen ───────────┐
 card on screen / opened    │   independent of
                            ▼
 unread ──mark read──▶ read        (Unseen and Unread are separate filters)
```

*Seen* answers “did I look at this card?”. *Read* answers “am I finished with
it?”. A seen, unread article stays in **Unread only**. **Unseen** hides what
you have already seen without changing read state.

## Library: the nav entry versus the eyebrow

- **Library** names exactly one thing: the article list and reader at
  `/articles`, reached from the **Library** navigation entry. The Library
  sidebar lists *View*, *Folders*, and *Tags*.
- Pages that are **not** the Library must not wear a “Library” eyebrow. The
  Briefing page used to say “Library”, the Alerts page too; they now say
  *Stories across your feeds* and *Saved searches*. Search pages use
  *Find articles* (not “Discover”, which belongs to the catalog).
- “Add to my library” is retired: you **add a feed** (“Add to my feeds”). The
  Library is where you *read* them.

## Page and menu names

A page's title and `<h1>` use the same label as the menu item that opens it, and
both come from the message catalog (`ui.text("…")`, `crates/core/src/i18n.rs`) so
that they translate together:

| Menu item | Key | Page title |
|---|---|---|
| Library | `library` | (scope name) |
| Search | `search` | Search |
| Saved searches | `saved_searches` | Saved searches |
| Alerts | `alerts` | Alerts |
| Briefing | `briefing` | Briefing |
| Roundups | `roundups` | Roundups |
| Discover | `discover` | Discover feeds |
| Feeds | `feeds` | Manage feeds |
| Account → Preferences | `preferences` | Preferences |
| Account → Customization | `customization` | Customization |
| Account → AI providers | `ai_providers` | Your AI providers |
| Account → Integrations | `integrations` | Integrations |
| Account → API tokens | `api_tokens` | API tokens |
| Account → Sessions | `sessions` | Sessions |
| Account → Passkeys | `passkeys` | Passkeys |
| Administration | `administration` | Overview, Users, Shared feeds, Blocklist & moderation, Abuse & activity, Feed access, Discover catalog, Background work, Costs, AI providers |

(`saved` — “Saved” — is the bookmark state of an article and must not be used
for the Saved searches menu item; the two were previously sharing one key.)

## Product taglines

The product one-liner (`docs/PRD.md` §1.1) is the single source for every
tagline:

> Your RSS reader, but it actually understands what you read — and it runs as a
> single fast binary you own.

| Surface | Wording |
|---|---|
| Landing `<title>`, social title, hero `<h1>` | “Your RSS reader, but it understands what you read.” |
| Landing eyebrow | “Self-hosted · one fast binary you own” |
| Landing closing eyebrow | “A single fast binary you own.” |
| Sign-in `<h1>` | “Your RSS reader, but it understands what you read.” |
| Sign-in eyebrow | “Welcome back” |

Retired: “Return to your signal.”, “Own the feed. Keep the signal.”, “RSS,
compiled differently”, “Feed magazine”.

## Internal terms that must not appear in visible text

Enforced by `crates/web/tests/vocabulary.rs` (element text and `title`,
`aria-label`, `placeholder`, `alt` attributes; not class names, URLs, comments,
or template expressions):

`lane`, `outbox`, `heartbeat`, `acquisition`, `agent`, `job`, `dead-letter`,
`embedding`, `vector`, `pgvector`, `hnsw`, `skip locked`, `advisory lock`,
`ssrf`, `onnx`, `singleton`, `cluster`, `digest`, `enrich`, `magazine`,
`glance`, `worker`.

Where an administrator genuinely needs one, the page keeps the term and
explains it next to it, and the test has a one-line `ALLOW` entry that names the
page and the term:

- **Background work & logs** and the **Overview**: *worker* (“a process that
  runs background tasks”).
- **Overview** storage panel: *embeddings*, *pgvector*, *HNSW* (explained in the
  tooltip as the search index), and *User agent* (the HTTP header, explained in
  the label).
- **Costs**: *embedding* (“calls that build the search index”).
- **Landing**: the public “how it is built” sections, for self-hosters who are
  evaluating the stack.

Everything else says the plain word from the table above.
