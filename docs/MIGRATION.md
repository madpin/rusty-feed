# Migration and upgrade guide

## Upgrading RustSS

Use the backup, stop, migrate, start, verify sequence in
[`OPERATIONS.md`](OPERATIONS.md). Migrations are embedded in every RustSS binary
and recorded in `_sqlx_migrations`; they are idempotent and forward-only.

Before an upgrade:

- export OPML as a human-readable subscription backup;
- create a PostgreSQL custom-format dump;
- preserve `ENCRYPTION_SECRET` and the model-cache volume;
- note the active `EMBEDDING_PROVIDER` and `EMBEDDING_MODEL`.

Do not change the embedding model as part of an ordinary binary upgrade. Model
changes require the explicit confirmed reindex workflow.

## Moving from NeuReed

RustSS does not read NeuReed's application database directly. The supported,
non-destructive path is:

1. Export OPML from NeuReed.
2. Create the RustSS users and let each user import their OPML from **Feeds**.
   The preview preserves nested folders and lets the user resolve duplicates.
3. Configure personal tags, content-source/lifetime preferences, and AI provider
   profiles in RustSS.
4. Keep NeuReed read-only until feeds have refreshed and users have verified
   their Library.

OPML carries subscriptions and folders, not read state, feedback, learned
patterns, summaries, API tokens, or secrets. RustSS deliberately does not copy
those incompatible private/security records. Shared feeds and articles dedupe
as users import the same sources, preserving the O(1) content rule.

## Moving to another RustSS host

Restore the PostgreSQL dump and model cache, use the original
`ENCRYPTION_SECRET`, configure the same embedding space, and start the new host
on a private address first. Verify readiness, counts, login, Library, search,
and one background job before changing DNS/reverse-proxy traffic.

Changing `SESSION_SECRET` signs everyone out safely. Changing
`ENCRYPTION_SECRET` without running a supported secret-rotation command makes
stored provider credentials unreadable; do not rotate it by substitution.
