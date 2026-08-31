# RustSS operations guide

This guide is the production runbook for a self-hosted RustSS instance. It
assumes PostgreSQL 17 with pgvector; Redis and Chromium are optional performance
and extraction tiers. PostgreSQL is always the source of truth.

## Deployment modes

- **Small instance:** one `rustss-server` with `RUN_EMBEDDED_WORKER=true`, plus
  PostgreSQL. Redis may be disabled with `CACHE_ENABLED=false`.
- **Scaled instance:** one or more web replicas with
  `RUN_EMBEDDED_WORKER=false`, one or more `rustss-worker` processes, PostgreSQL,
  optional Redis, and the remote Chromium service.
- **Compose:** `docker compose --env-file .env -f docker/compose.yaml up -d`.
  Keep the model volume persistent so restarts do not download ONNX weights.

Never run both an embedded worker and an unintended standalone worker merely to
“make jobs move.” Multiple workers are supported, but capacity should be an
intentional choice visible under **Administration → Queue & logs**.

## Startup and health

1. Run `rustss-cli config-check`; its output is safe to attach to a support case.
2. Run `rustss-cli migrate` before changing application replicas. The container
   entrypoint does this by default when `AUTO_MIGRATE=true`.
3. Check `/api/health` for process liveness and `/api/health/ready` for database
   readiness. Redis, LLM, and Chromium failures do not make core reading
   unavailable.
4. Check `/metrics` and **Administration → Queue & logs** for worker heartbeats,
   lane capacity, waiting/running/stalled/dead work, recent completions, and
   sanitized failures.

### systemd

Example hardened units for the embedded-worker and split-worker layouts live in
`contrib/systemd/`. They use `Type=notify`, `NotifyAccess=main`, and a 30-second
watchdog. RustSS reports ready only after migrations plus listener/worker
startup, reports stopping before it drains, and pings at half the watchdog
period. Without `NOTIFY_SOCKET`, the feature creates no timer or task. Set
`SYSTEMD_NOTIFY_ENABLED=false` to suppress it explicitly.

Use `systemctl status rustss` to see the current status text and
`journalctl -u rustss -f` for structured process logs. In the split layout, set
`RUN_EMBEDDED_WORKER=false` before enabling `rustss-worker.service`.

### TLS termination and HTTP/2

The supported bundled TLS boundary is Caddy, not ad-hoc certificate code in the
Rust binary. Use exactly one Compose profile: `https-auto` for ACME/Let's
Encrypt or `https-custom` for a read-only mounted full chain and private key.
Set `BASE_URL=https://…` and `APP_BIND_IP=127.0.0.1`; only Caddy should bind
public ports 80/443. Certificate/config state for automatic mode is persistent.

Before reloading a custom chain, run `caddy validate`; after atomic file
replacement, run `caddy reload`. Confirm expiry with `openssl s_client ... |
openssl x509 -noout -dates` and HTTP/2 with `openssl s_client -alpn h2` (the
output must contain `ALPN protocol: h2`). Keep the prior chain/key until the
reload and external probe succeed. `scripts/test-tls-proxy.sh` exercises the
same pinned image and ALPN boundary in CI.

### Trusted proxy identity

Before enabling reverse-proxy authentication, firewall the RustSS listener to
the proxy network and configure the proxy to strip any inbound header matching
`REVERSE_PROXY_AUTH_USER_HEADER` before it writes verified identity. Use the
proxy's immediate CIDR, not a user-controlled forwarded-address header. Start
with `REVERSE_PROXY_AUTH_AUTO_PROVISION=false`, verify an existing non-admin
account, then enable provisioning if desired. Map admin emails sparingly: the
mapping applies only at first creation and does not silently overwrite later
database role changes. Configure the upstream logout URL if ending a RustSS
session must also terminate SSO.

The Administration overview reports enablement, trust-network count, domain and
admin mapping counts, provisioning, and logout configuration without exposing
the actual identity values. Test spoof resistance from a non-proxy address
before production cutover and keep a local admin recovery account.

## Canonicalize legacy article URLs

Preview tracking-parameter cleanup without changing data:

```bash
rustss-cli canonicalize-urls
```

The report separates collision-free changes from existing-key and many-to-one
collisions. Apply only the safe set with `rustss-cli canonicalize-urls --apply`.
Collisions remain untouched for an administrator to review; RustSS never merges
personal state or shared enrichment implicitly.

## Backups

Back up PostgreSQL and the model cache. Redis and Chromium have no authoritative
state. Use a custom-format dump so individual tables can be inspected:

```bash
pg_dump --format=custom --no-owner --no-privileges \
  --dbname="$DATABASE_URL" --file=rustss-$(date +%F).dump
```

For Compose, run the client in the database container and stream the dump to the
host:

```bash
docker compose --env-file .env -f docker/compose.yaml exec -T postgres \
  pg_dump -U rustss -Fc rustss > rustss-$(date +%F).dump
```

Encrypt backups at rest. Store `ENCRYPTION_SECRET` and `SESSION_SECRET` in a
separate secret manager: the database dump intentionally contains encrypted AI
keys, but it cannot decrypt them without the original encryption secret.

## Restore rehearsal

Restore into a new, empty database—not over the live database:

```bash
createdb rustss_restore
pg_restore --no-owner --no-privileges --dbname=rustss_restore rustss.dump
DATABASE_URL=postgres://.../rustss_restore rustss-cli migrate
```

Verify `_sqlx_migrations`, user/feed/article counts, one authenticated Library
page, search, and a queued feed refresh before switching traffic. The release
rehearsal restored 100,000 articles, 100,000 embeddings, and the then-current
33 migrations from a 7.5 MiB custom dump. A second rehearsal on 2026-08-31
restored the current 41-migration schema and matched all source counts (178
users, 327 feeds, 123 articles, and 1,776 subscriptions) from a 312 KiB custom
dump; the release image then served an authenticated Library page from the
restored database. See `RELEASE_EVIDENCE.md`.

## Safe upgrade and rollback

1. Read release notes and back up PostgreSQL.
2. Stop standalone workers, then web replicas. This prevents old code from
   claiming work after a forward migration.
3. Pull the immutable version tag or digest; verify its Sigstore signature.
4. Run the new `rustss-cli migrate` once.
5. Start workers, then web replicas; check readiness and queue heartbeats.
6. Run a Library/search/API smoke test.

Database migrations are forward-only. Application rollback is safe only when
the release notes say the previous binary understands the migrated schema.
Otherwise restore the pre-upgrade dump into a new database and switch the
connection URL. Never edit `_sqlx_migrations` manually.

## Queue maintenance

The admin queue page exposes bounded controls:

- **Scan due feeds** enqueues one shared feed-refresh scan.
- **Queue cleanup** enqueues shared content/retention maintenance.
- **Recover stalled jobs** releases jobs locked for more than ten minutes.
- **Retry dead letters** retries at most 100 recent failed jobs.

All controls enqueue or update durable work; the web request never performs a
publisher crawl. Inspect the error class before retrying repeated failures.

## Dependency failures

- **Redis:** RustSS continues with its local cache and PostgreSQL. Cross-instance
  invalidation resumes after a bounded subscriber reconnect.
- **Chromium:** extraction keeps the local Readability/raw result. Publisher
  URLs still pass through `SafeFetchClient`; Chromium never navigates around the
  SSRF boundary.
- **LLM:** interactive summaries remain in the priority lane, errors appear in
  admin logs, and the cascade tries user, administrator, then environment
  providers. Core reading is unaffected.
- **Worker termination:** a missing heartbeat appears as stalled. The scheduled
  or manual reclaim path returns expired leases to pending without duplicating
  shared content.

## Models and reindexing

Run `rustss-cli fetch-model` during provisioning and persist `MODEL_CACHE_DIR`.
For an air-gapped host, copy the populated directory before startup. Changing
`EMBEDDING_MODEL` is an explicit maintenance event:

1. stop workers;
2. change the provider/model configuration;
3. run `rustss-cli reindex --confirm`;
4. restart workers and monitor the background lane;
5. run `rustss-cli backfill` if an interrupted migration left gaps.

The database guard rejects a mixed model/dimension space instead of silently
corrupting similarity.

## Capacity and retention

The dashboard reports database, article/index, embedding/HNSW, and shared media
storage. `ARTICLE_RETENTION_DAYS=0` is infinite. A finite policy never deletes an
article bookmarked by any user. Public-page copies have their own
subscriber-owned finite/infinite lifetime; RSS content remains stored.

Alert on database disk growth, no worker heartbeat, dead-letter growth, repeated
feed failures, and non-zero HTTP 5xx rates. Use the route-template metrics from
`/metrics`; raw article IDs are intentionally not metric labels.

## Release verification

Tagged releases publish checksummed native archives for Linux/macOS on amd64 and
arm64, Linux Debian/RPM packages, per-architecture SPDX SBOMs, GitHub build
provenance, and an SBOM/provenance-bearing `linux/amd64,linux/arm64` image.
GitHub OIDC signs every file and registry digest with Sigstore/cosign. GHCR is
mandatory; repository variables plus credentials may mirror the exact digest to
Docker Hub and Quay. CI installs the Debian package twice and boots the released
container against PostgreSQL before creating the GitHub release. Pin production
to a version or digest rather than `latest`.

Packages intentionally do not enable the service or create live secrets. Copy
`/usr/share/doc/rustss/rustss.env.example` to `/etc/rustss/rustss.env`, replace
all placeholder secrets/URLs, run `rustss-cli config-check`, then enable the
service. Package upgrades preserve `/etc/rustss`, apply forward migrations on
startup, and retain `/var/lib/rustss`; use the backup/rollback rules above.
