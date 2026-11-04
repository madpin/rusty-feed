# Rusty Feed user guide

Rusty Feed is a multi-user reader with one simple ownership rule: downloaded feed
content is shared efficiently, while subscriptions, folders, tags, read state,
bookmarks, feedback, saved searches, preferences, credentials, and public-share
policies belong to you.

This guide covers the v1 web application. Administrators should also keep the
[`OPERATIONS.md`](OPERATIONS.md) runbook nearby.

## 1. Sign in and orient yourself

Open the Rusty Feed address supplied by your administrator. You can sign in with a
local email/password or an enabled OAuth provider. On an open instance, the
first registered account becomes the administrator. Regular accounts start with
an empty library and choose sources in the setup wizard, Discover catalog, OPML
import, or direct-add flow. Only temporary demo accounts receive the instance's
featured showcase feeds; those are pre-organized into topic folders. Invitation-only
instances require a one-use invite.

Local accounts can use **Forgot password** on the sign-in page. The reset link lasts one hour, works once, and signs out every existing session. The instance has to be able to send email; otherwise an administrator sets the new password.

The main navigation contains:

- **Library** — your article list and reader;
- **Discover** — the curated feed magazine;
- **Feeds** — subscriptions, folders, tags, OPML, and bulk actions;
- **Search** — keyword, semantic, or hybrid discovery;
- **Saved searches** — persistent queries and matches;
- **Account** — reading preferences, customization, AI providers, integrations,
  passkeys, sessions, and API tokens;
- **Administration** — visible only to administrators.

Your account menu also shows unread notifications. Rusty Feed updates the count in
place and groups repeated feed-health changes so they do not overwhelm you.

## 2. Read from the Library

The Library sidebar is stable navigation, independent of the current article
filter. Use it to switch between:

- **Most recent**, **Unread only**, **Unseen**, or **Personalized** ranking;
- **Unread AI summaries** — jump straight to the summaries that finished generating while you moved on to other articles, skipping ones you've already read or that don't have a summary yet;
- a folder and all of its nested sources;
- **Unfiled feeds**;
- one source;
- one tag across every subscribed feed.

**Unread only** is the default for new accounts. Choose **Most recent** to include read items for the current view, or enable **Show read articles** in Preferences to make that the saved default.

Article cards expose read/unread, save, AI summary, and thumbs-up/down actions.
Thumbs feedback is private and trains your personal relevance model. A bookmark
is also personal and protects the shared article from age-based retention.

Use **Mark all read** to apply the current Library scope—not the whole account
unless the current scope is the whole account. The Older link keeps the active
filter. With JavaScript, further pages append as you reach the end of the list.

Opening an article marks it seen. That is separate from read: a seen, unread
card stays in the unread list unless you also mark it read. **Unseen** hides
articles you have already opened. **Briefing** collects stories that more than
one of your sources covered.

### Reading modes

Choose a mode under **Account → Reading & display**:

- **Side panel** keeps the article list and article in independent, full-height
  scroll regions. Resize the divider with pointer or keyboard.
- **Inline** expands an article beneath its card while keeping list context.
- **Standalone** opens a focused article page.

All modes use the same sanitized content, personal state, summary, sharing, and
media controls. Eight color palettes—including Rusty's warm oxide palette and
live system light/dark—combine with sans-serif or serif typography. Font family, size, line height, article
density, sidebar width, and reader-panel width are personal settings rendered
before JavaScript loads.

The article itself is one continuous reading surface. Source metadata and the
primary actions form one compact header; public-share settings open from that
same toolbar; an available AI summary appears as the article's **At a glance**
preamble; attachments and video follow the same section rhythm before the
publisher body. The standalone, side-panel, and anonymous shared views use the
same typography and spacing, while private controls and reading state never
appear on a public link.

### RSS, processed content, and Live View

Each subscription chooses its preferred reading source. The article reader's
**RSS / Page content / Live** switch changes only the current article. When that
temporary view differs from the subscription, choose **Make default for this
feed** to persist it for later articles from that feed. Opening the next or
previous article always resolves that article's saved feed default instead of
carrying the temporary override forward:

- **RSS content** is always stored and remains the fallback.
- **Page content** asks Rusty Feed to fetch and extract the publisher's public
  page once for everyone who requests it. Choose a finite lifetime or **No
  expiration**. The longest subscriber request controls the one shared copy.
- **Live** fetches the current publisher document when an article opens and
  displays it through an article-scoped, credentialless sandbox. It creates no
  durable page-extraction demand and falls back to RSS without JavaScript or on
  failure.

If page content is missing or expired, opening the article immediately queues a
priority, revision-deduplicated refresh and shows RSS content until it is ready.
A temporary Page Content view retains that shared copy for one day without
changing the feed default; a permanent feed default uses its configured
lifetime. Choosing Live uses the
same SSRF-safe fetcher and rewrites required static resources through scoped
tickets, avoiding publisher iframe blocks while keeping Rusty Feed cookies out
of the frame. Live replaces only the article body, so the article header,
actions, and RSS/Page/Live switch stay in the same place as Page Content. Its
window uses the browser's available dynamic viewport height, allowing the
publisher document to scroll inside a full-screen reading area. Close
Live View at any time to return to stored reader content. An attachment-only
item without a public page remains visibly in the feed's Live mode but explains
that it is showing RSS as the required fallback. Live is less private than
Reader View, so use it when the original layout or interactive presentation
matters.

## 3. Organize feeds without leaving the Library

Open **Feeds** to create hierarchical folders and manage many subscriptions.
Folder names are unique among siblings, so `Work / News` and `Personal / News`
can coexist.

For one source, choose **Organize** to set:

- one folder or **Unfiled**;
- a personal display name;
- any number of normalized tags;
- content source and public-page lifetime;
- optional display/media overrides.

The same source settings page has a personal **Entry filters** JSON editor. A
rule has a name, target (`title`, `content`, `author`, or `url`), mode (`include`
or `exclude`), bounded regular expression, optional case-insensitive flag, and
enabled flag. Use **Preview rules** first: Rusty Feed reports how many of the 20 most
recent shared articles would disappear from your Library. The filter changes
only your view; the shared article remains available to every other subscriber.

Administrators also see **Shared rewrite rules** for that source. Ordered rules
can remove a CSS-selected element or replace selected text with a bounded regex.
Every save creates a restorable version and runs the sanitizer afterward. A
change forces one shared rebuild of current publisher feed entries and requested
public pages; RSS remains available while the queue catches up. Restoring an old
version creates a new audit version rather than rewriting history.

The Library sidebar has a **Quick feed tools** switch. When enabled, compact
buttons open content and tag dialogs without navigating away. Folders can be
collapsed and the whole sidebar can be reduced; both states follow your account
across browsers. Selecting a tag clears feed/folder/unread/summary intersections and the implicit
hide-read preference, matches catalog and personal tag casing consistently,
loads all read and unread subscribed articles under that tag, and updates only the article-results
region so the sidebar itself is not rebuilt or repositioned.

To move a feed while reading, drag the article's source handle onto a sidebar
folder. Keyboard and touch users can activate the same handle to open an
accessible folder picker. The operation moves the source subscription, never a
copy of the article.

### Touch navigation

Touch gestures are optional and disabled by default. In **Settings → Reading
preferences → Touch gestures**, enable horizontal article navigation, choose
low/medium/high sensitivity, and select natural or reversed direction. A swipe
left opens the next article in natural mode. Vertical scrolling always takes
priority, and swipes that start on links, buttons, forms, media, or dialogs are
ignored. Gestures never perform destructive actions or silently change saved
state; the same navigation remains available through article links and keyboard
shortcuts.

Rusty Feed adapts the same workflows across desktop, tablet, portrait, and landscape
screens. On smaller screens the three-column reader becomes a single list/page,
the Library sidebar moves into a **Library navigation** disclosure, forms and
media controls stack, and wide operational tables scroll inside their own
panel. Device safe areas are respected. No horizontal page scrolling should be
needed; if an individual article or admin table is wider, only that bounded
region scrolls.

### Bulk operations

Select subscriptions on **Feeds** to move them, add/remove/replace tags, queue a
refresh, pause/resume acquisition, or unsubscribe with confirmation. Operations
are scoped to your subscriptions and report partial failures instead of hiding
successful changes. Unsubscribing removes personal state; it does not delete a
shared feed or its content for other subscribers.

## 4. Discover, add, import, and export feeds

**Discover** contains at least 200 curated sources. Filter automatically by
category, country, and language; cards show observed article count, RSS body
size, cadence, last post, and subscription state. Select several cards and
subscribe once. **Preview** distinguishes RSS examples from the separate
processed-content availability report.

To add any public feed, use **Add your own feed URL**. Rusty Feed validates and
normalizes the URL, prevents private-network access by default, and reuses an
existing shared feed row when another user already follows it.

### Subscribe bookmarklet

On **Feeds → Subscribe from any website**, drag **Subscribe with Rusty Feed** to
your bookmarks bar, or copy the shown bookmarklet address into a new bookmark.
Activate it while visiting a publisher. Rusty Feed opens a separate confirmation
page and lists the RSS, Atom, or JSON feeds advertised by that page; choose one
and confirm. It never subscribes on the publisher page itself. You must already
be signed in to Rusty Feed in that browser; if your session expired, sign in and run
the bookmarklet again.

Discovery is anonymous and bounded: it sends no site cookies, runs no publisher
JavaScript, rejects private-network redirects under the instance policy, and
reads at most 2 MiB. When no feed is advertised, paste a known feed URL into
**Add a feed** instead. Assign its folder and tags after subscribing.

### OPML

From **Feeds**, upload an OPML file or supply a public OPML URL. The preview:

1. normalizes and deduplicates up to 2,000 subscriptions;
2. shows new versus already-subscribed sources;
3. preserves nested folders;
4. lets you select individual sources or folder groups;
5. offers keep/replace behavior for existing personal names and folders.

The result reports every success and failure. **Export OPML** creates a portable
file containing subscriptions, nested folders, and personal source names. OPML
does not contain passwords, API keys, read state, feedback, or summaries.

## 5. Search and saved searches

Search supports three modes:

- **Keyword** uses PostgreSQL full-text indexes and supports quoted phrases,
  uppercase `OR`, implicit `AND`, and `-excluded` terms.
- **Semantic** searches by meaning in the active, guarded embedding space.
- **Hybrid** combines both and keeps exact keyword matches visible even while
  an embedding is missing.

Choose an instance-inherited default or personal mode, text stemming, and
hybrid meaning weight under preferences. If embedding a query fails, Rusty Feed
continues with keyword results and explains the fallback.

A saved search accepts boolean groups, phrases, `AND`, `OR`, and `NOT`. Set its
threshold and notification policy, then edit, run, paginate, or rematch it from
**Saved searches**. New shared articles are evaluated without duplicating
content per user; only matches and notifications are personal.

## 6. Generate AI summaries

Choose **Summarize with AI** on an article card or reader. The button stays live:
it reports queued, generating, retrying, ready, or failed state without making
you refresh the page. Interactive summaries use reserved queue capacity so feed
downloads cannot starve an action you are waiting for.

The output contains a short summary, key points, and topics. It is stored once
for the shared article revision; the cost ledger records who requested it and
which provider/model was used.

Under **Account → AI providers**, add prioritized personal OpenAI profiles. API
keys are encrypted and never shown again. To rotate a key, delete and recreate
the profile. When a provider fails, Rusty Feed tries enabled profiles in this order:

1. your personal profiles by priority;
2. administrator-managed instance profiles;
3. the environment-configured provider.

Administrators can configure OpenAI or trusted Ollama-compatible endpoints.
On the same instance AI page, they can enable automatic summaries and choose the
minimum normalized plain-text article size (2,000 characters by default). Feed
ingestion applies changes to created or materially updated articles without
restarting workers. Existing eligible articles
are not queued merely by saving: **Backfill existing articles** is a separate,
explicit bounded background action. Manual per-article summaries ignore the
automatic threshold. Users cannot supply arbitrary endpoints. Every exhausted
failure is sanitized and visible both beside the article and in the administrator
queue/log view.

## 7. Privacy, media, and sharing

Rusty Feed strips common tracking parameters, removes configured pixel trackers,
sets no-referrer policy, sanitizes publisher HTML, blocks external scripts from
stored reader content, and adds opener isolation to external links. You can show
the number of removed tracking images for each selected content source.

Enclosure audio, video, images, and downloads can use the same-origin shared
media proxy. Global attachment preferences can be inherited or reset per feed
from **Feeds → source settings**, including proxy/direct delivery and
on-demand/metadata/links-only loading. Nothing is fetched merely because a second subscriber opens the
article. Player policy can require explicit consent and always retains a
publisher link.

The article's header image follows that same effective per-feed media-delivery
choice. In **proxy** mode it is fetched through an authenticated, SSRF-safe
Rusty Feed URL with an 8 MiB ceiling and a passive raster-image MIME allow-list, so
the browser never contacts the publisher. **Mixed content** proxies only HTTP
images; **direct** mode may contact the publisher and still sends no referrer.

Recognized YouTube links stay iframe-free in stored content. If a publisher
supplies only an official YouTube iframe, Rusty Feed preserves its validated
video identity as an inert watch link before removing the active iframe. When
enabled, an explicit **Load player** action creates the selected validated player frame,
preserves the linked start time, and offers unload/retry. The built-in choice
uses only `youtube-nocookie.com`, and no player request is made before consent.
If the administrator configures a named
Invidious-compatible HTTPS player, select it under Reading & display; its exact
origin is validated and CSP-bound, and removing it safely restores the built-in
privacy player. Subscription settings may force the built-in player or links
only.

If disabled-by-default duration enrichment is enabled by the administrator,
Rusty Feed fetches a recognized video's public metadata once, within a whole-instance
daily budget, and shows the cached duration beside that shared video. Player
consent remains separate: duration metadata does not load an iframe.

### Public article links

From an article's sharing controls, create a revocable bearer link with:

- a finite lifetime or administrator-permitted no-expiration policy;
- RSS or available public-page body;
- title/metadata/source-link controls;
- optional external images, with a privacy warning.

Creating or updating rotates the 256-bit token. Rusty Feed stores a lookup hash and
encrypted token rather than plaintext, copies no article body, performs no
visitor-triggered extraction, and serves the public page as no-store/noindex.
Revoke the link to invalidate it immediately.

## 8. Preferences, shortcuts, sessions, and installation

**Account → Reading & display** contains theme, typography, density, reading
mode, the ADHD-friendly format, daily briefing options, sidebar, external-link,
media, YouTube, privacy, search, notification, keyboard, locale, fallback
locale, IANA time zone, date order, and number grouping settings. Reset/inherit controls make the effective instance default
clear. The shell, navigation, primary reader actions, and formatted dates use
the selected profile; Arabic also switches the document to right-to-left.
Some feature/admin/validation strings still fall back to English while the
remaining translation catalogs are completed.

External links can use the privacy-clean or publisher-provided URL and open in
the current tab or a new tab. The administrator may require new tabs. Every
combination still suppresses referrers and opener access; that security policy
cannot be disabled.

Arrow keys navigate articles and feeds by default. Shortcut capture rejects
duplicates and unsafe/conflicting choices before saving. Core actions still
work through normal links and forms when JavaScript is unavailable.

**Account → Sessions** lists active sign-ins and lets you revoke another device
or all other sessions. **API tokens** are revocable app credentials; copy a new
token immediately because Rusty Feed stores only its hash.

### Passkeys

When enabled by the administrator, open **Account → Passkeys** to add Touch ID,
Windows Hello, a hardware security key, or a password-manager passkey. Local
accounts confirm their current password; passwordless/OAuth accounts must have
signed in during the last 15 minutes. You may name and keep up to 20 credentials.
The login page asks for your email first, then opens the authenticator prompt;
the error is intentionally the same for an unknown account and an account with
no passkey.

Revocation also requires a session created in the last 15 minutes. Sign out and
back in when asked. Rusty Feed will not remove the last passkey from an account with
no other recovery method. It stores only the public credential and counter
state—never a fingerprint, face scan, or other biometric material.

### Interface customization and Safe Mode

Open **Account → Customization** for versioned personal CSS. External imports,
resource URLs, and legacy active CSS constructs are rejected; preview before
saving, reset at any time, or restore an older version as a new one.
Administrator-managed JavaScript is separately disabled by default. When an
administrator publishes a reviewed script, each user must explicitly opt in
under Reading & display. It is served as a same-origin external asset without
weakening the no-inline CSP.

If either customization makes the interface unusable, open `/safe-mode`
directly on your Rusty Feed origin. Safe Mode suppresses both assets before rendering
and provides a link to leave the mode after you reset or repair the version.

Rusty Feed can be installed from **Reading & display** when the browser exposes an
install prompt. Otherwise follow the shown browser-menu/Add to Home Screen
instructions. The service worker caches only public CSS, JavaScript, and icons;
it never caches authenticated pages, articles, APIs, media, or Live View.

## 9. Mobile clients and automation

New integrations should use the versioned native API documented in
[`API.md`](API.md). Create a token under **Account → API tokens** and send it as
`Authorization: Bearer ...` over HTTPS.

Existing RSS clients can use the implemented Google Reader-compatible subset.
Set the server URL to the Rusty Feed origin and authenticate with a local password
or app API token. OAuth-only accounts use an app token. Subscription, folder,
stream, read, star, and mark-all-read operations map to the same personal state
as the web UI.

Fever-compatible clients use the same Rusty Feed origin and a named Fever credential
created under **Account → API tokens**. The adapter covers groups, feeds,
favicons, items, unread/saved identifiers, and read/save changes. It does not
implement historical Fever links, sparks, or kindlings; see [`API.md`](API.md)
for the exact supported subset.

### Article destinations and event webhooks

Open **Account → Integrations** to create multiple named destinations. Choose a
provider shape, SSRF-safe HTTPS endpoint, no auth/Bearer/Basic/HMAC credential,
message template, tags, events, retry attempts, rate limit, and optional default.
Credentials are encrypted and never redisplayed; use **Rotate credential** to
replace one. Test, pause/resume, delete, and recent delivery history are on the
same page.

From an article, **Send to…** opens a destination picker over the reader, so
your article and reading position stay in place while the delivery is queued.
The default destination is preselected. Choose another active destination when
needed; if none exists, the picker links to the larger Integrations setup flow.
With JavaScript disabled, the same control opens an equivalent full page.
Automatic events are available for bookmark, new feed entries, and summary
ready. Signed webhooks include payload/event version headers and an
`X-Rusty-Feed-Signature: sha256=…` over the exact request body. Failed destinations
retry durably and then become visible dead letters without affecting reading.
Discord, Slack, Telegram, Ntfy, and signed webhooks have native payload shapes;
other catalog entries should be treated as generic compatible endpoints until
their exact hosted-service contract is marked complete in the parity tracker.

## 10. Administration

The administration area separates configuration from live operations:

- **Overview** shows accounts, shared feeds/articles, active public shares,
  database/article/embedding/media storage, worker count/capacity, and queue
  totals.
- **Users** changes roles with self-demotion and last-admin protection, clears
  another account's learned ranking, and issues invitation links when
  registration is invite-only.
- **Feeds** shows shared health, cadence, failures, pause state, and safe actions.
- **Catalog** publishes, orders, and previews curated sources.
- **AI providers** manages prioritized instance fallback profiles.
- **Acquisition profiles** creates and assigns one shared, host-pinned policy
  for an authorized cookie, explicit HTTP(S)/SOCKS proxy, or temporary invalid
  certificate exception. Cookies require a path and expiry; certificate
  exceptions require expiry. Values are encrypted, never redisplayed, and safe
  assignment/profile facts appear in the audit table. Never use this feature to
  bypass a paywall or access control.
- **Queue & logs** is a wide, auto-refreshing operations view with lane/kind
  depth, scheduled/running/stalled/dead work, live heartbeats, recent
  completions, and sanitized actionable errors.

Queue maintenance buttons are deliberately bounded: scan due feeds, queue
cleanup, recover locks older than ten minutes, or retry at most 100 dead
letters. A web request never performs the publisher crawl itself.

## 11. Troubleshooting

- **“Showing feed-provided content…”** means public-page extraction is queued or
  retrying. RSS remains available. Check the live state, then ask an
  administrator to inspect the extraction job if it remains failed.
- **AI summary failed**: open its live failure message. An administrator can see
  the same sanitized error, lane, attempts, and provider fallback history.
- **A feed is stale**: check whether it is paused, then use **Refresh selected**.
  Repeated failures move to daily and eventually weekly checks rather than
  hammering the publisher.
- **Live View is blank**: clear and reload it. If the publisher or a required
  resource is unavailable, continue in Reader View or open the no-referrer
  original link.
- **Semantic search is unavailable**: keyword search remains functional. An
  administrator should verify the active model and background embedding lane.
- **The install button is missing**: use the browser's install/Add to Home
  Screen menu; browser support is intentionally described rather than guessed.
- **Customization broke the page**: navigate directly to `/safe-mode`, reset or
  restore the CSS/script from Customization, then leave Safe Mode.
- **A passkey action asks you to sign in again**: revocation and passwordless
  enrollment require a session created within the last 15 minutes; this is an
  intentional recent-auth boundary.
- **A destination is retrying/dead**: inspect Account → Integrations for its
  HTTP status and sanitized error. Test the endpoint and rotate credentials;
  reading and feed acquisition are unaffected.

For deployment, backups, model changes, queues, and dependency outages, use the
[`OPERATIONS.md`](OPERATIONS.md) runbook. For configuration ownership and safe
defaults, use [`CONFIGURATION.md`](CONFIGURATION.md).
