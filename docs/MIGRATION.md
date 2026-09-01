# Migration and upgrade guide

## Upgrading Rusty Feed

Use the backup, stop, migrate, start, verify sequence in
[`OPERATIONS.md`](OPERATIONS.md). Migrations are embedded in every Rusty Feed binary
and recorded in `_sqlx_migrations`; they are idempotent and forward-only.

Before an upgrade:

- export OPML as a human-readable subscription backup;
- create a PostgreSQL custom-format dump;
- preserve `ENCRYPTION_SECRET` and the model-cache volume;
- note the active `EMBEDDING_PROVIDER` and `EMBEDDING_MODEL`.

Do not change the embedding model as part of an ordinary binary upgrade. Model
changes require the explicit confirmed reindex workflow.

### 0.1 pre-release identity rename

The Rusty Feed rename is a coordinated namespace reset, not an ordinary binary
upgrade. It changes Cargo packages and binaries, OS packages and services,
default filesystem paths, the Compose project and volumes, cookie names, Redis
keys, webhook headers/payloads, and encrypted-credential domain labels.

For any instance created before this rename:

1. Export OPML and take a PostgreSQL dump before stopping the earlier stack.
2. Start the renamed stack with fresh volumes and restore the PostgreSQL dump,
   or keep an explicit `DATABASE_URL` pointed at the existing database. Compose
   will not automatically discover volumes created under a different project
   name.
3. Copy the existing model cache into `MODEL_CACHE_DIR`, or run
   `rusty-feed-cli fetch-model` again.
4. Expect every browser session to be signed out. Recreate stored LLM,
   integration, and acquisition credentials; rotate public share links; resend
   any pending email-verification link. The renamed encryption domains cannot
   decrypt ciphertext created by a pre-rename build, even with the same
   `ENCRYPTION_SECRET`.
5. Update webhook consumers to accept the `X-Rusty-Feed-*` headers and the
   `rusty-feed` generic payload key.
6. Replace package/service automation with `rusty-feed-server`,
   `rusty-feed-worker`, `rusty-feed-cli`, `rusty-feed.service`, and
   `rusty-feed-worker.service`. Configuration-file discovery now uses
   `RUSTY_FEED_CONFIG_FILE`; TLS and bootstrap helpers use the matching
   `RUSTY_FEED_*` names documented in `CONFIGURATION.md`.

This reset is intentionally confined to the unpublished 0.1 release-candidate
line. Once a stable release is published, future identity or storage-format
changes require an automated compatibility migration.

## Moving from NeuReed

Rusty Feed does not read NeuReed's application database directly. The supported,
non-destructive path is:

1. Export OPML from NeuReed.
2. Create the Rusty Feed users and let each user import their OPML from **Feeds**.
   The preview preserves nested folders and lets the user resolve duplicates.
3. Configure personal tags, content-source/lifetime preferences, and AI provider
   profiles in Rusty Feed.
4. Keep NeuReed read-only until feeds have refreshed and users have verified
   their Library.

OPML carries subscriptions and folders, not read state, feedback, learned
patterns, summaries, API tokens, or secrets. Rusty Feed deliberately does not copy
those incompatible private/security records. Shared feeds and articles dedupe
as users import the same sources, preserving the O(1) content rule.

## Moving to another Rusty Feed host

Restore the PostgreSQL dump and model cache, use the original
`ENCRYPTION_SECRET`, configure the same embedding space, and start the new host
on a private address first. Verify readiness, counts, login, Library, search,
and one background job before changing DNS/reverse-proxy traffic.

Changing `SESSION_SECRET` signs everyone out safely. Changing
`ENCRYPTION_SECRET` without running a supported secret-rotation command makes
stored provider credentials unreadable; do not rotate it by substitution.
