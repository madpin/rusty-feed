# Rusty Feed operations guide

This guide is the production runbook for a self-hosted Rusty Feed instance. It
assumes PostgreSQL 17 with pgvector; Redis and Chromium are optional performance
and extraction tiers. PostgreSQL is always the source of truth.

## Deployment modes

- **Small instance:** one `rusty-feed-server` with `RUN_EMBEDDED_WORKER=true`, plus
  PostgreSQL. Redis may be disabled with `CACHE_ENABLED=false`.
- **Scaled instance:** one or more web replicas with
  `RUN_EMBEDDED_WORKER=false`, one or more `rusty-feed-worker` processes, PostgreSQL,
  optional Redis, and the remote Chromium service.
- **Compose:** `docker compose --env-file .env -f docker/compose.yaml up -d`.
  Keep the model volume persistent so restarts do not download ONNX weights.

Never run both an embedded worker and an unintended standalone worker merely to
“make jobs move.” Multiple workers are supported, but capacity should be an
intentional choice visible under **Administration → Background work** (the admin page says *tasks* and *priority*; see `docs/GLOSSARY.md` for the mapping to jobs and lanes).

## Startup and health

1. Run `rusty-feed-cli config-check`; its output is safe to attach to a support case.
2. Run `rusty-feed-cli migrate` before changing application replicas. The container
   entrypoint does this by default when `AUTO_MIGRATE=true`.
3. Check `/api/health` for process liveness and `/api/health/ready` for database
   readiness. Redis, LLM, and Chromium failures do not make core reading
   unavailable.
4. Check `/metrics` and **Administration → Background work** for worker heartbeats,
   lane capacity, waiting/running/stalled/dead work, recent completions, and
   sanitized failures.

### systemd

Example hardened units for the embedded-worker and split-worker layouts live in
`contrib/systemd/`. They use `Type=notify`, `NotifyAccess=main`, and a 30-second
watchdog. Rusty Feed reports ready only after migrations plus listener/worker
startup, reports stopping before it drains, and pings at half the watchdog
period. Without `NOTIFY_SOCKET`, the feature creates no timer or task. Set
`SYSTEMD_NOTIFY_ENABLED=false` to suppress it explicitly.

Use `systemctl status rusty-feed` to see the current status text and
`journalctl -u rusty-feed -f` for structured process logs. In the split layout, set
`RUN_EMBEDDED_WORKER=false` before enabling `rusty-feed-worker.service`.

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

Response compression: `rusty-feed-server` gzips HTML, CSS/JS, JSON and SVG
itself when `HTTP_COMPRESSION=true` (the default) and the client sends
`Accept-Encoding: gzip`, so a bare run or a Traefik/Dokploy route without proxy
compression still sends small pages. Images, audio, video, fonts, archives,
range-capable media proxy bodies and already-encoded responses are never
re-compressed, so ranges stay byte-exact. The bundled Caddy profiles already
use `encode zstd gzip`; Caddy leaves a response that carries `Content-Encoding`
alone, so enabling both is safe but wasteful. Set `HTTP_COMPRESSION=false` when
the proxy owns compression (for example to get zstd from Caddy).

### Trusted proxy identity

Before enabling reverse-proxy authentication, firewall the Rusty Feed listener to
the proxy network and configure the proxy to strip any inbound header matching
`REVERSE_PROXY_AUTH_USER_HEADER` before it writes verified identity. Use the
proxy's immediate CIDR, not a user-controlled forwarded-address header. Start
with `REVERSE_PROXY_AUTH_AUTO_PROVISION=false`, verify an existing non-admin
account, then enable provisioning if desired. Map admin emails sparingly: the
mapping applies only at first creation and does not silently overwrite later
database role changes. Configure the upstream logout URL if ending a Rusty Feed
session must also terminate SSO.

The Administration overview reports enablement, trust-network count, domain and
admin mapping counts, provisioning, and logout configuration without exposing
the actual identity values. Test spoof resistance from a non-proxy address
before production cutover and keep a local admin recovery account.

## Canonicalize legacy article URLs

Preview tracking-parameter cleanup without changing data:

```bash
rusty-feed-cli canonicalize-urls
```

The report separates collision-free changes from existing-key and many-to-one
collisions. Apply only the safe set with `rusty-feed-cli canonicalize-urls --apply`.
Collisions remain untouched for an administrator to review; Rusty Feed never merges
personal state or shared enrichment implicitly.

## Backups

Back up PostgreSQL and the model cache. Redis and Chromium have no authoritative
state. Use a custom-format dump so individual tables can be inspected:

```bash
pg_dump --format=custom --no-owner --no-privileges \
  --dbname="$DATABASE_URL" --file=rusty-feed-$(date +%F).dump
```

For Compose, run the client in the database container and stream the dump to the
host:

```bash
docker compose --env-file .env -f docker/compose.yaml exec -T postgres \
  pg_dump -U rusty_feed -Fc rusty_feed > rusty-feed-$(date +%F).dump
```

Encrypt backups at rest. Store `ENCRYPTION_SECRET` and `SESSION_SECRET` in a
separate secret manager: the database dump intentionally contains encrypted AI
keys, but it cannot decrypt them without the original encryption secret.

## Restore rehearsal

Restore into a new, empty database—not over the live database:

```bash
createdb rusty_feed_restore
pg_restore --no-owner --no-privileges --dbname=rusty_feed_restore rusty-feed.dump
DATABASE_URL=postgres://.../rusty_feed_restore rusty-feed-cli migrate
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
4. Run the new `rusty-feed-cli migrate` once.
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

Finished work records are purged by the scheduled `queue_maintenance` agent,
which runs on `CLEANUP_SCHEDULE` under a singleton lease:

- `done` queue jobs and dispatched outbox events older than `JOB_RETENTION_DAYS`
  (default 7) are deleted;
- finished (`success`/`failed`) agent run records older than
  `AGENT_RUN_RETENTION_DAYS` (default 30) are deleted;
- `0` disables the purge for that table. `pending`, `running` and dead-letter
  jobs, unprocessed outbox events and `running` agent runs are never deleted;
  dead letters stay until an operator retries them. A finished AI summary job
  whose article still has no summary is also kept, so the reader keeps showing
  why that summary is missing.

Deletes run in batches of 5,000 rows with at most 500,000 rows per table per
run. The first run after upgrading deletes the existing backlog in batches and
may need several nights if the backlog is larger than that cap. PostgreSQL
reuses the freed space for new rows; the files shrink only after `VACUUM FULL`
or a dump/restore, which is optional.

## Dependency failures

- **Redis:** Rusty Feed continues with its local cache and PostgreSQL. Cross-instance
  invalidation resumes after a bounded subscriber reconnect. Search page results are
  cached per user for 5 minutes in either tier. A repeated search does not embed the
  query again or count against the daily meaning-search cap, and newly fetched articles
  can take up to 5 minutes to appear in it. Subscription changes clear the cache
  immediately, and keyword-fallback pages are never cached (FND-233). Library unread
  counts are cached per reader for `UNREAD_COUNT_CACHE_SECONDS` (30 by default);
  new articles can take that long to appear in the counts, while a reader's own
  actions show at once. Set it to `0` to recompute on every page load.
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

Run `rusty-feed-cli fetch-model` during provisioning and persist `MODEL_CACHE_DIR`.
For an air-gapped host, copy the populated directory before startup. Changing
`EMBEDDING_MODEL` is an explicit maintenance event:

1. stop workers;
2. change the provider/model configuration;
3. run `rusty-feed-cli reindex --confirm`;
4. restart workers and monitor the background lane;
5. run `rusty-feed-cli backfill` if an interrupted migration left gaps.

The database guard rejects a mixed model/dimension space instead of silently
corrupting similarity.

## Database connection pool

Every process (server with embedded worker, standalone worker, CLI) owns one
PostgreSQL pool of `DATABASE_MAX_CONNECTIONS` (default 10). Requests and
background jobs share it; a caller that cannot get a connection within
`DATABASE_ACQUIRE_TIMEOUT_SECONDS` (default 30) fails.

Rule of thumb per process: concurrent web requests that hit the database at the
same time + worker concurrency (all lanes when the worker is embedded) + a few
connections of headroom for health checks and migrations. Across the cluster,
keep the sum over every process of `DATABASE_MAX_CONNECTIONS`, plus about eight
per server for media fetch locks, plus any other clients (`psql`, backups,
poolers), below PostgreSQL `max_connections` (100 by default). Raise the value
if logs show acquire timeouts while Postgres is idle; lower it, or put PgBouncer
in transaction mode in front, when many replicas would exceed
`max_connections`. The media proxy no longer holds a pooled connection during
upstream downloads, so a page of new thumbnails cannot exhaust the pool; it
holds at most eight short-lived dedicated lock connections per server.

## Capacity and retention

The dashboard reports database, article/index, embedding/HNSW, and shared media
storage. The Articles total on the dashboard and Shared feeds page is approximate (shown
with a leading `~`); it comes from database statistics refreshed by autovacuum,
so it can lag slightly behind recent changes. Lists, filters and pagination use
exact counts. `ARTICLE_RETENTION_DAYS=0` is infinite. A finite policy never deletes an
article bookmarked by any user. Public-page copies have their own
subscriber-owned finite/infinite lifetime; RSS content remains stored.

Alert on database disk growth, no worker heartbeat, dead-letter growth, repeated
feed failures, and non-zero HTTP 5xx rates. Use the route-template metrics from
`/metrics`; raw article IDs are intentionally not metric labels.

## PostgreSQL tuning

`docker/compose.dokploy.yaml` starts PostgreSQL with conservative settings read
from environment variables, so you tune without editing the file. Set them in the
Dokploy service environment (or `.env`) and redeploy. Defaults are safe on a 2 GB
host. Durability (`fsync`, `synchronous_commit`, `full_page_writes`) and
`max_connections` are not touched.

| Variable | Default | Meaning |
| --- | --- | --- |
| `PG_SHARED_BUFFERS` | `512MB` | PostgreSQL page cache; about 25% of host RAM. |
| `PG_EFFECTIVE_CACHE_SIZE` | `1536MB` | Planner hint for OS plus PostgreSQL cache; 50-75% of RAM. |
| `PG_WORK_MEM` | `16MB` | Memory per sort/hash node, per connection. |
| `PG_MAINTENANCE_WORK_MEM` | `128MB` | Index builds, vacuum. |
| `PG_RANDOM_PAGE_COST` | `1.1` | SSD/NVMe. Set `4` for spinning disks. |
| `PG_JIT` | `off` | Standard recommendation for short OLTP queries; not measured for this workload. |
| `PG_SHM_SIZE` | `256mb` | Container `/dev/shm`; Docker's 64 MB default breaks parallel hash joins once `work_mem` grows. |

Suggested values by host RAM (PostgreSQL shares the host with the app, Redis, and
Chromium, so these are deliberately below the full-host rule of thumb):

| Host RAM | `PG_SHARED_BUFFERS` | `PG_EFFECTIVE_CACHE_SIZE` | `PG_WORK_MEM` | `PG_MAINTENANCE_WORK_MEM` | `PG_SHM_SIZE` |
| --- | --- | --- | --- | --- | --- |
| 2 GB | `512MB` | `1536MB` | `16MB` | `128MB` | `256mb` |
| 4 GB | `1GB` | `3GB` | `16MB` | `256MB` | `256mb` |
| 8 GB | `2GB` | `6GB` | `32MB` | `512MB` | `512mb` |
| 16 GB | `4GB` | `12GB` | `64MB` | `1GB` | `1gb` |

`work_mem` is per sort or hash node, per connection, and one query can use
several nodes, so the worst case is roughly connections x nodes x `work_mem`.
Raise it only after checking the app pool size and `max_connections`.

Confirm the settings took effect with
`docker exec <postgres container> psql -U rusty_feed -c "SHOW shared_buffers;"`
(repeat for the other settings). Changing `shared_buffers` and `shm_size` needs
a Postgres restart, so redeploy the service in Dokploy.

To verify before and after: run the keyword search from `crates/db/src/search.rs`
under `EXPLAIN (ANALYZE, BUFFERS)` right after a restart, then run it again.
`Buffers: shared read=` should drop to near zero on the second run, and a larger
`shared_buffers` keeps it there for a bigger working set.

## Release verification

Tagged releases publish checksummed native archives for Linux on amd64 and arm64 and macOS on Apple
silicon (arm64), Linux Debian/RPM packages, per-architecture SPDX SBOMs, GitHub build
provenance, and an SBOM/provenance-bearing `linux/amd64,linux/arm64` image.
GitHub OIDC signs every file and registry digest with Sigstore/cosign. GHCR is
mandatory; repository variables plus credentials may mirror the exact digest to
Docker Hub and Quay. Linux binaries are built on Ubuntu 24.04 and need glibc
2.39 or newer (Debian 13, Ubuntu 24.04 and later); CI rejects any build that
needs more. CI installs the Debian package twice on Ubuntu 24.04 and Debian 13,
and boots the released container against PostgreSQL before creating the GitHub
release. Pin production to a version or digest rather than `latest`.

Packages intentionally do not enable the service or create live secrets. Copy
`/usr/share/doc/rusty-feed/rusty-feed.env.example` to `/etc/rusty-feed/rusty-feed.env`, replace
all placeholder secrets/URLs, run `rusty-feed-cli config-check`, then enable the
service. Package upgrades preserve `/etc/rusty-feed`, apply forward migrations on
startup, and retain `/var/lib/rusty-feed`; use the backup/rollback rules above.
