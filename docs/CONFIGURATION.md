# RustSS configuration reference

RustSS reads one typed configuration at startup and refuses to start when a
required value is missing or a value is outside its documented bounds. This
page covers application settings; Docker Compose interpolation variables are
listed separately at the end.

## Sources and precedence

Configuration is resolved in this order, from highest to lowest precedence:

1. Process environment variables.
2. Values loaded from `.env` when the process starts in a directory containing
   that file. Existing process variables are never overwritten.
3. An optional TOML file named by `RUSTSS_CONFIG_FILE`.
4. Built-in defaults.

TOML keys are lower-case snake case versions of the environment names:

```toml
bind_addr = "0.0.0.0:8080"
database_url = "postgres://rustss:password@postgres/rustss"
session_secret = "replace-with-at-least-32-random-bytes"
encryption_secret = "replace-with-at-least-32-random-bytes"
cache_enabled = false
registration_mode = "closed"
```

An explicitly configured TOML path must exist. Relative paths are resolved from
the process working directory. `RUSTSS_CONFIG_FILE` itself is environment-only.

Run this before starting a production instance:

```bash
cargo run -p rustss-cli -- config-check
```

The command validates the resolved values and prints a support-friendly JSON
summary. It prints presence flags for credentials and connection strings, never
their values.

## Required settings

| Environment variable | Purpose | Validation |
|---|---|---|
| `DATABASE_URL` | PostgreSQL 17/pgvector connection URL | Required and non-empty |
| `SESSION_SECRET` | Session signing and short-lived ticket key material | At least 32 bytes |
| `ENCRYPTION_SECRET` | Master material for encrypted stored credentials | At least 32 bytes |

Generate independent secrets, for example with `openssl rand -base64 48`. Never
reuse the development defaults from Compose in a reachable deployment.

## Server, branding, and public sharing

| Variable | Default | Meaning |
|---|---:|---|
| `BIND_ADDR` | `0.0.0.0:8080` | Listener address |
| `BASE_URL` | derived from `BIND_ADDR` | Public origin used for redirects, links, and WebAuthn relying-party identity |
| `LOG_LEVEL` | `info` | `tracing` filter such as `info` or `rustss=debug` |
| `SYSTEMD_NOTIFY_ENABLED` | `true` | Auto-detect a systemd notify socket and emit readiness, stopping, and watchdog messages; set false to suppress all notifications |
| `PWA_NAME` | `RustSS` | Installed-app name, 1–64 printable characters |
| `PWA_SHORT_NAME` | `RustSS` | Home-screen name, 1–32 printable characters |
| `PWA_THEME_COLOR` | `#2563eb` | Manifest color in `#RRGGBB` form |
| `PWA_BACKGROUND_COLOR` | `#f8fafc` | Manifest launch color in `#RRGGBB` form |
| `EXTERNAL_LINKS_NEW_TAB_REQUIRED` | `false` | Override personal same-tab navigation for publisher links |
| `PUBLIC_ARTICLE_SHARING_ENABLED` | `true` | Permit creation of revocable public links |
| `PUBLIC_SHARE_DEFAULT_LIFETIME_DAYS` | `30` | Default finite share lifetime |
| `PUBLIC_SHARE_MAX_LIFETIME_DAYS` | `3650` | Maximum finite lifetime, 1–3650 |
| `PUBLIC_SHARE_ALLOW_INFINITE` | `true` | Offer an explicit non-expiring share option |
| `API_ENABLED` | `true` | Enable the native `/api/v1` resource API (health and metrics are unaffected) |
| `GREADER_API_ENABLED` | `true` | Enable `/accounts/ClientLogin` and `/reader/api/0/*` |
| `FEVER_API_ENABLED` | `true` | Enable the JSON-only `/fever/` adapter and Fever credential creation |

In production, set `BASE_URL` to the external HTTPS origin. Keep TLS at the
reverse proxy and forward requests to `BIND_ADDR` over the private network.

## Interface customization

| Variable | Default | Meaning |
|---|---:|---|
| `CUSTOM_CSS_ENABLED` | `true` | Allow bounded versioned personal CSS without imports or external resource URLs |
| `CUSTOM_JAVASCRIPT_ENABLED` | `false` | Allow an administrator-managed same-origin script; each user must still opt in |
| `UI_CUSTOMIZATION_MAX_BYTES` | `65536` | Per CSS/script version, 1,024–262,144 bytes |

Saved CSS and JavaScript versions live in PostgreSQL and are included in normal
backups. JavaScript is treated like application code: only administrators can
publish it, credentials are not involved, and it never gains an inline CSP
exception. `/safe-mode` suppresses both assets before the customized page is
rendered, so a user can recover without editing the database.

### Bundled HTTPS profiles

Compose includes two mutually exclusive Caddy profiles. Both terminate modern
TLS/HTTP/2 and proxy to `app:8080`; RustSS itself stays protocol-simple.

| Variable | Automatic | Custom | Meaning |
|---|---:|---:|---|
| `APP_BIND_IP` | `127.0.0.1` | `127.0.0.1` | Keep the optional host HTTP port local while Caddy owns public ports |
| `RUSTSS_DOMAIN` | required | required | Public DNS name and certificate SNI |
| `TLS_EMAIL` | required | unused | ACME account/expiry contact |
| `TLS_CERT_DIR` | unused | `../certs` | Host directory mounted read-only at `/certs` |
| `TLS_CERT_FILE` | unused | `cert.pem` | Certificate chain filename inside the directory |
| `TLS_KEY_FILE` | unused | `key.pem` | Private-key filename inside the directory |

Set `BASE_URL=https://$RUSTSS_DOMAIN`. Activate exactly one of `https-auto` or
`https-custom`; an empty domain or invalid/missing custom certificate makes
Caddy fail instead of exposing a misleading TLS endpoint.

## Cache

| Variable | Default | Meaning |
|---|---:|---|
| `CACHE_ENABLED` | `true` | Enable optional Redis L2 caching |
| `REDIS_URL` | unset | Redis connection URL; omit for L1-only operation |
| `REDIS_PASSWORD` | unset | Password supplied separately when required |
| `CACHE_NAMESPACE` | `rustss` | 1–64 ASCII letters, digits, `.`, `-`, or `_` |

Use a different namespace for every RustSS installation sharing one Redis.
Redis is disposable: PostgreSQL remains the source of truth, and a Redis outage
degrades the instance to its in-process cache.

## Trusted reverse-proxy authentication

Disabled by default. RustSS trusts an identity header only when the immediate
TCP peer—not `X-Forwarded-For`—belongs to an explicit CIDR. Keep the application
unreachable except through that proxy; the bundled TLS proxy does not set
identity headers by itself.

| Variable | Default | Meaning |
|---|---:|---|
| `REVERSE_PROXY_AUTH_ENABLED` | `false` | Enable the trusted-header path alongside local/OAuth sessions |
| `REVERSE_PROXY_AUTH_TRUSTED_CIDRS` | loopback IPv4/IPv6 | Comma-separated immediate-peer networks |
| `REVERSE_PROXY_AUTH_USER_HEADER` | `x-remote-user` | Header carrying a verified email address |
| `REVERSE_PROXY_AUTH_NAME_HEADER` | `x-remote-name` | Optional display-name header |
| `REVERSE_PROXY_AUTH_AUTO_PROVISION` | `true` | Create a personal account on first trusted request; false requires an existing account |
| `REVERSE_PROXY_AUTH_ALLOWED_DOMAINS` | empty | Optional comma-separated exact email-domain allow-list |
| `REVERSE_PROXY_AUTH_ADMIN_EMAILS` | empty | Exact emails granted admin only when first provisioned; later roles remain database-owned |
| `REVERSE_PROXY_AUTH_LOGOUT_URL` | empty | Absolute application path or credential-free HTTPS upstream logout endpoint |

Configure the upstream to delete/overwrite any client-supplied identity header
before authentication, then set it from verified identity. RustSS independently
ignores the header from untrusted peers. A local session takes precedence, so
break-glass local administration remains available.

## Registration and OAuth

| Variable | Default | Meaning |
|---|---:|---|
| `REGISTRATION_MODE` | `open` | `open`, `invite`, or `closed` |
| `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET` | unset | Enable Google login when both are set |
| `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET` | unset | Enable GitHub login when both are set |
| `OAUTH_CLIENT_ID`, `OAUTH_CLIENT_SECRET` | unset | Generic OAuth/OIDC client |
| `OAUTH_AUTH_URL` | unset | Generic provider authorization endpoint |
| `OAUTH_TOKEN_URL` | unset | Generic provider token endpoint |
| `OAUTH_USERINFO_URL` | unset | Generic provider profile endpoint |
| `OAUTH_PROVIDER_NAME` | unset | Display name for the generic provider |
| `PASSKEYS_ENABLED` | `false` | Enable WebAuthn registration and passwordless sign-in; requires a pure HTTPS `BASE_URL` origin, except loopback development |
| `PASSKEY_RP_NAME` | `RustSS` | WebAuthn relying-party display name, 1–64 printable characters |

Local password accounts work without OAuth. The first registered account is
promoted to administrator. In `invite` mode, administrators issue one-use,
expiring links from **Administration → Users**.

When passkeys are enabled, users may register multiple named credentials under
**Account → Passkeys**. A password account must confirm its current password
before registration; passwordless/OAuth accounts require a session created in
the last 15 minutes. Revocation always requires that recent session. Challenge
state is short-lived, stored server-side, and consumed once; RustSS stores the
public credential and counter state, never biometric material. `BASE_URL` may
not contain credentials, a path, query, or fragment. Keep a second
login/recovery method before revoking the last credential. WebAuthn attestation verification uses the platform OpenSSL 3
library; outbound HTTP and OAuth traffic continue to use rustls.

## Embeddings and search

| Variable | Default | Meaning |
|---|---:|---|
| `EMBEDDING_PROVIDER` | `local` | `local` or `openai` |
| `EMBEDDING_MODEL` | `BAAI/bge-small-en-v1.5` | One active vector space per instance |
| `EMBEDDING_BATCH_SIZE` | `10` | Positive processing batch size |
| `EMBEDDING_AUTO_GENERATE` | `false` | Queue embeddings during ingestion |
| `MODEL_CACHE_DIR` | `/var/lib/rustss/models` | Persistent model directory |
| `OPENAI_API_KEY` | unset | Required for the OpenAI embedding provider |
| `OPENAI_BASE_URL` | provider default | OpenAI-compatible API base URL |
| `SEARCH_MODE` | `hybrid` | `hybrid`, `keyword`, or `semantic` |
| `SEARCH_TEXT_CONFIG` | `simple` | `simple` multilingual tokens or `english` stemming |
| `SEARCH_SEMANTIC_WEIGHT_PERCENT` | `60` | Semantic contribution to hybrid ranking, 0–100 |

Run `rustss-cli fetch-model` while online to populate `MODEL_CACHE_DIR`; copy
that directory when preparing an offline host. Normal startup durably guards
the active model and dimension. To change spaces, stop workers, update the
configuration, run `rustss-cli reindex --confirm`, and restart them. Use
`rustss-cli backfill` to queue only missing current-space vectors.

## Retention

| Variable | Default | Meaning |
|---|---:|---|
| `ARTICLE_RETENTION_DAYS` | `0` | `0` keeps article rows forever; otherwise delete old rows after this many days |
| `RETENTION_AGE_BASIS` | `published` | Age from publisher date (with ingestion fallback), or from `created` ingestion time |

Retention is global because articles are shared. An article bookmarked by any
user is always preserved. Public-page copies have a separate longest-requested
subscriber lifetime and RSS content is retained when those copies expire.

## Summarization

| Variable | Default | Meaning |
|---|---:|---|
| `LLM_PROVIDER` | `openai` | `openai` or `ollama` |
| `LLM_SUMMARY_MODEL` | `gpt-4o-mini` | Article-summary model |
| `LLM_DIGEST_MODEL` | `gpt-4o-mini` | Digest model |
| `LLM_AUTO_SUMMARIZE` | `false` | Queue one background summary per shared article revision; may incur provider cost |
| `OLLAMA_BASE_URL` | `http://localhost:11434` | Ollama/OpenAI-compatible endpoint |

The environment provider is the final fallback. Users may add personal OpenAI
profiles under **Account → AI providers**; administrators may add instance-wide
OpenAI or Ollama-compatible profiles under **Administration → AI providers**.
The enabled cascade is personal profiles by priority, then administrator
profiles, then environment configuration. API keys are encrypted with
domain-separated AEAD and are never redisplayed; rotate one by deleting and
recreating its profile. User profiles use the fixed official OpenAI endpoint,
while only trusted administrators may configure a compatible endpoint. Core
reading does not depend on an LLM; failures remain visible in the live summary
state, interactive lane, and wide admin operations view.

## Fetching, extraction, and privacy

| Variable | Default | Meaning |
|---|---:|---|
| `CHROMIUM_ENABLED` | `true` | Allow the headless-browser extraction tier |
| `CHROMIUM_CDP_URL` | unset | Remote Chrome DevTools endpoint |
| `EXTRACTION_TIMEOUT_MS` | `30000` | Bounded extraction request time |
| `EXTRACTION_DELAY_MS` | `3000` | Per-domain politeness delay |
| `RESPECT_ROBOTS_TXT` | `true` | Enforce publisher robots policy |
| `OUTBOUND_USER_AGENT` | `RustSS/0.1 (+https://github.com/madpin/rustss)` | 1–256 printable ASCII characters sent by the shared publisher client |
| `OUTBOUND_HTTP1_ONLY` | `false` | Disable outbound HTTP/2 and force HTTP/1.1 |
| `FEEDS_ALLOW_PRIVATE_NETWORK` | `false` | Permit private/loopback targets; trusted networks only |
| `TRACKING_URL_CLEANUP_ENABLED` | `true` | Strip built-in analytics parameters |
| `TRACKING_URL_EXTRA_PARAMETERS` | empty | Comma-separated exact names or trailing-wildcard prefixes |
| `TRACKING_URL_DOMAIN_EXCEPTIONS` | empty | Comma-separated host exceptions, including subdomains |
| `TRACKING_PIXEL_STRICTNESS` | `balanced` | `conservative`, `balanced`, or `strict` |
| `TRACKING_PIXEL_BLOCKED_HOSTS` | empty | Up to 100 comma-separated tracker hosts |
| `TRACKING_PIXEL_ALLOWED_HOSTS` | empty | Up to 100 comma-separated image hosts |
| `YOUTUBE_EMBEDS_ENABLED` | `true` | Offer consent-gated privacy-enhanced players |
| `YOUTUBE_SOURCE_HOSTS` | built-in YouTube hosts | Bounded comma-separated accepted source hosts |
| `YOUTUBE_ALTERNATIVE_PLAYER_NAME` | empty | 1–40 character label for an optional alternative player |
| `YOUTUBE_ALTERNATIVE_PLAYER_URL` | empty | Credential-free HTTPS Invidious-compatible base URL |
| `YOUTUBE_DURATION_ENABLED` | `false` | Queue shared public-page duration enrichment for recognized videos |
| `YOUTUBE_DURATION_DAILY_BUDGET` | `100` | Whole-instance attempts per UTC day, 1–10,000 |
| `YOUTUBE_DURATION_CACHE_DAYS` | `30` | Shared ready/failed metadata lifetime, 1–365 days |

Leave private-network fetching disabled on internet-facing instances. Every
publisher fetch is still routed through the SSRF-safe client and bounded by the
application's content limits. The user agent and protocol setting apply
consistently to feeds, public pages, robots, icons, Live View resources, and
proxied media; Administration displays the effective values. Header controls
cannot contain control/non-ASCII bytes, so configuration cannot inject another
request header.

Administrators configure sensitive per-feed acquisition under
**Administration → Acquisition profiles**, not through environment variables.
Profiles may contain an encrypted authorized cookie, explicit HTTP(S)/SOCKS
proxy, and/or invalid-certificate exception. Host pins are always required;
cookies also require an absolute path pin and 1–365 day expiry, while certificate
exceptions require expiry. Assignment, deletion, and safe profile facts are
audited. A profile is shared by all subscribers and must not bypass paywalls or
publisher access controls.

Users configure article destinations and signed webhooks under
**Account → Integrations**. Their encrypted credentials, templates, events,
retry/rate policy, and delivery history are database state covered by backups;
no additional environment variable is needed.

## Shared enclosure media

| Variable | Default | Validation/meaning |
|---|---:|---|
| `MEDIA_PROXY_ENABLED` | `true` | Enable article-authorized shared delivery |
| `MEDIA_PROXY_MAX_BYTES` | `67108864` | 1 MiB–512 MiB per object |
| `MEDIA_PROXY_CACHE_BUDGET_BYTES` | `2147483648` | At least object max; no more than 100 GiB |
| `MEDIA_PROXY_CACHE_TTL_SECONDS` | `604800` | 60–2,592,000 seconds |
| `MEDIA_PROXY_NEGATIVE_TTL_SECONDS` | `300` | 10–3,600 seconds |
| `MEDIA_PROXY_EXTRA_INLINE_MIME_TYPES` | empty | Comma-separated subset of the documented passive-only extension list: `audio/3gpp`, `audio/3gpp2`, `audio/x-m4a`, `image/heic`, `image/heif`, `image/jxl`, `video/3gpp`, `video/3gpp2`, `video/x-m4v`. Active document types are always rejected. |

Media bodies are shared by source URL, never copied per subscriber. PostgreSQL
cleanup enforces the configured lifetime and aggregate budget.

## Scheduler and workers

| Variable | Default | Meaning |
|---|---:|---|
| `ENABLE_CRON_JOBS` | `true` | Enable scheduled enqueueing |
| `RUN_EMBEDDED_WORKER` | `true` | Consume jobs in the web process |
| `FEED_REFRESH_SCHEDULE` | `*/5 * * * *` | Validated five-field UTC cron schedule for the due-feed scan |
| `CLEANUP_SCHEDULE` | `0 3 * * *` | Validated five-field UTC cron schedule for retention/cleanup |
| `FEED_REFRESH_CONCURRENCY` | `3` | Positive background-lane capacity |
| `INTERACTIVE_JOB_CONCURRENCY` | `1` | Positive reserved interactive capacity |
| `SAVED_SEARCH_CONCURRENCY` | `5` | Reserved saved-search worker capacity |
| `JOB_LOCK_TIMEOUT_MS` | `600000` | Reserved configurable lease duration |

The two queue lanes ensure a user-requested summary is not starved by feed
downloads. In scaled mode, disable the embedded worker on web replicas and run
one or more `rustss-worker` processes against the same PostgreSQL database.

## Docker Compose-only variables

These values configure containers or port publishing and are not fields in
`AppConfig`: `APP_PORT`, `POSTGRES_PORT`, `REDIS_PORT`, `CHROMIUM_PORT`, and
`AUTO_MIGRATE`. Always invoke Compose with the project env file:

```bash
docker compose --env-file .env -f docker/compose.yaml config --quiet
docker compose --env-file .env -f docker/compose.yaml up -d
```

The Compose app service deliberately overrides internal connection URLs so
host-published ports do not leak into container-to-container networking.
