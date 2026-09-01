# Getting Started with Rusty Feed

This guide walks you through installing and running **Rusty Feed** — step by step, no
Rust knowledge required. Pick the option that matches where you want to run it:

- [A. Localhost with Docker Compose](#a-localhost-with-docker-compose-easiest) — easiest, recommended
- [B. Localhost from source](#b-localhost-from-source-for-developers) — for developers / contributors
- [C. Dokploy](#c-dokploy) — one-click-ish self-hosted PaaS
- [D. Proxmox](#d-proxmox) — home-lab / self-hosted server

> **Current status (v1 release candidate).** The complete reading, discovery,
> search, AI, API, and administration application is implemented and validated.
> The repository has not been publicly tagged from this worktree; production
> operators should deploy an immutable maintainer-published version tag/digest
> rather than an arbitrary branch build.

---

## Before you start: generate your secrets

Rusty Feed needs two secret values (at least 32 characters each). You'll paste these
into a configuration file in the steps below. Generate them now:

```bash
# Run each command and copy the output somewhere safe.
openssl rand -base64 48   # use this for SESSION_SECRET
openssl rand -base64 48   # use this for ENCRYPTION_SECRET
```

- **SESSION_SECRET** — required instance secret; current sessions are opaque,
  database-backed tokens and the value is reserved for session-key rotation.
- **ENCRYPTION_SECRET** — required master secret for encrypted stored-secret
  paths. Public article-page extraction is anonymous and never uses feed or user
  cookies. Keep it safe and do not change it after first run unless you have a
  rotation plan.

No `openssl`? Any 32+ character random string works (e.g. from a password
manager).

---

## A. Localhost with Docker Compose (easiest)

This runs everything — the app, the PostgreSQL database, Redis, and a headless
Chrome — in containers. You don't need to install Rust.

### 1. Install Docker
Install **Docker Desktop** (macOS/Windows) or **Docker Engine + Compose plugin**
(Linux). Verify:

```bash
docker --version
docker compose version
```

### 2. Get the code

```bash
git clone https://github.com/madpin/rusty-feed.git
cd rusty-feed
```

### 3. Create your configuration file
Copy the example and fill in your secrets:

```bash
cp .env.example .env
```

Open `.env` in a text editor and set at least these two lines to the values you
generated above:

```bash
SESSION_SECRET=<paste the first openssl output>
ENCRYPTION_SECRET=<paste the second openssl output>
```

Everything else has sensible defaults. (Optional: add `OPENAI_API_KEY=...` later
for AI summaries, or leave AI on the free local mode.)

### 4. Start everything

```bash
docker compose --env-file .env -f docker/compose.yaml up -d
```

The **first** run builds the app and downloads images, so it can take several
minutes. Later starts are fast. `--env-file .env` is required because the
Compose file lives under `docker/`; it supplies root `.env` values for Compose
interpolation, and the app service also reads that file at runtime.

> **Port already in use?** If you see an error about port `5432`, `6379`, `9222`,
> or `8080` being allocated, something else on your machine uses it. Set an
> alternative in your `.env`, for example:
> ```bash
> POSTGRES_PORT=55432
> REDIS_PORT=56379
> CHROMIUM_PORT=59222
> APP_PORT=58080
> ```
> (Set `APP_PORT` to change the app's published port, and update `BASE_URL` to
> match when it is externally visible.)

### 5. Check it's running

```bash
curl http://localhost:8080/api/health
# {"data":{"service":"Rusty Feed","status":"ok","version":"0.1.0"}}
```

Open <http://localhost:8080/api/health> in your browser too.

### 6. Everyday commands

```bash
docker compose --env-file .env -f docker/compose.yaml logs -f app     # watch app logs
docker compose --env-file .env -f docker/compose.yaml ps              # what's running
docker compose --env-file .env -f docker/compose.yaml down            # stop (keeps data)
docker compose --env-file .env -f docker/compose.yaml up -d --build   # rebuild after updating code
docker compose --env-file .env -f docker/compose.yaml down -v         # stop AND delete all data ⚠️
```

Database migrations run automatically on startup (`AUTO_MIGRATE=true`).

> **PostgreSQL 16 upgrade:** PostgreSQL 17 cannot use an existing PostgreSQL 16
> `pgdata` volume in place. Back up and restore into a new PostgreSQL 17 volume,
> or use `pg_upgrade`, before changing an existing deployment. Do not delete the
> old volume unless you have confirmed its backup and migration.

### Redis-free, single-process mode

Redis is optional. The `rusty-feed-server` process embeds the worker by default, so
PostgreSQL is the only required service for a small single-node deployment:

```bash
CACHE_ENABLED=false docker compose --env-file .env -f docker/compose.yaml up -d app
```

For a directly installed binary, set `DATABASE_URL`, `SESSION_SECRET`, and
`ENCRYPTION_SECRET`, then run `CACHE_ENABLED=false rusty-feed-server`. This uses the
in-process cache and embedded worker; it does not require Redis or Chromium.
SQLite is not yet a supported database backend.

---

## B. Localhost from source (for developers)

Use this if you want to change the code. You'll run the app with Rust's build
tool, but still use Docker for the database (much easier than installing it by
hand). Redis is optional, and the current extractor does not require Chrome.

### 1. Install the tools
- **Rust** (the toolchain is pinned in the repo, this installs it):
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  # then restart your terminal, or run: source "$HOME/.cargo/env"
  rustc --version
  ```
- **Docker** (for the dependencies) — as in Option A.

### 2. Get the code and configure

```bash
git clone https://github.com/madpin/rusty-feed.git
cd rusty-feed
cp .env.example .env       # then edit .env and set SESSION_SECRET + ENCRYPTION_SECRET
```

### 3. Start the dependencies

```bash
# Minimal development stack: PostgreSQL only.
docker compose --env-file .env -f docker/compose.yaml up -d postgres

# Optional Redis L2 cache and bounded Chromium extraction fallback.
docker compose --env-file .env -f docker/compose.yaml up -d redis chromium
```

Make sure your `.env` `DATABASE_URL` points at this Postgres. With the default
compose port that's:

```bash
DATABASE_URL=postgres://rusty_feed:rusty_feed@localhost:5432/rusty_feed
# Redis L2 is optional. For L1-only operation, omit REDIS_URL and set CACHE_ENABLED=false.
REDIS_URL=redis://localhost:6379
```

(If you changed `POSTGRES_PORT`, update the port in `DATABASE_URL` to match.)

### 4. Create the database schema

```bash
cargo run -p rusty-feed-cli -- migrate
# -> "migrations applied"
```

### 5. Run the app

```bash
cargo run -p rusty-feed-server
```

Then, in another terminal:

```bash
curl http://localhost:8080/api/health
```

Stop the app with `Ctrl-C`.

### 6. Handy developer commands

```bash
cargo test                                           # run tests
cargo fmt --all                                      # format code
cargo clippy --all-targets --all-features -- -D warnings   # lint
cargo run -p rusty-feed-worker                           # run background workers (once implemented)
```

<details>
<summary>Running PostgreSQL (and optionally Redis) natively</summary>

You can install PostgreSQL 17 with the **pgvector** extension directly on your
machine instead of using Docker, then point `DATABASE_URL` at it. Redis is
optional: omit `REDIS_URL` and set `CACHE_ENABLED=false` for L1-only caching.
pgvector must be installed and the database must allow `CREATE EXTENSION vector`
(Rusty Feed's migration does this for you). Docker is recommended because it bundles
the tested Postgres + pgvector version for you.
</details>

---

## C. Dokploy

[Dokploy](https://dokploy.com) is a self-hosted platform that deploys Docker apps
from a Git repo. Rusty Feed ships a Compose file it can use directly.

### 1. Prepare
- Have Dokploy installed on your server (see Dokploy's own install docs).
- Push your copy of Rusty Feed to a Git repository Dokploy can reach (GitHub, GitLab,
  or a private repo with credentials).

### 2. Create a Compose service
1. In the Dokploy dashboard, create a **Project**, then add a **Compose** service.
2. Point it at your Rusty Feed Git repository and branch.
3. Set the **Compose file path** to `docker/compose.yaml`.

### 3. Set environment variables (secrets)
In the service's **Environment** settings, add:

```
SESSION_SECRET=<your generated secret>
ENCRYPTION_SECRET=<your generated secret>
```

Optional: `OPENAI_API_KEY`, `LOG_LEVEL=info`, and any OAuth credentials.

### 4. Expose the app
- Point Dokploy's domain/proxy at the **`app`** service, container port **8080**.
- Add your domain and enable HTTPS (Dokploy handles the certificate).
- Set `BASE_URL=https://your-domain` in the environment so links are correct.

### 5. Deploy
Click **Deploy**. Dokploy builds the image and starts the stack; migrations run
automatically on boot. Verify at `https://your-domain/api/health`.

> Data (Postgres) lives in the compose volumes Dokploy manages — configure
> backups for the `pgdata` volume.

---

## C.1 Built-in HTTPS with Docker Compose

Rusty Feed ships optional Caddy profiles; choose exactly one. For automatic
Let's Encrypt certificates, point public DNS at the host, allow inbound TCP 80
and TCP/UDP 443, then set:

```dotenv
BASE_URL=https://reader.example.com
APP_BIND_IP=127.0.0.1
RUSTY_FEED_DOMAIN=reader.example.com
TLS_EMAIL=admin@example.com
```

Start the automatic profile:

```bash
docker compose --env-file .env -f docker/compose.yaml \
  --profile https-auto up -d
```

Caddy redirects HTTP to HTTPS, obtains and renews the certificate, and
negotiates HTTP/2. Its certificate state lives in named volumes. Keep the
application's optional port 8080 bound to loopback as shown.

For an existing certificate, put the full chain and unencrypted private key in
a root-readable host directory, set `TLS_CERT_DIR`, `TLS_CERT_FILE`, and
`TLS_KEY_FILE`, then use `--profile https-custom`. After atomically replacing
the two files, validate and reload without restarting Rusty Feed:

```bash
docker compose --env-file .env -f docker/compose.yaml \
  exec caddy-custom caddy validate --config /etc/caddy/Caddyfile
docker compose --env-file .env -f docker/compose.yaml \
  exec caddy-custom caddy reload --config /etc/caddy/Caddyfile
openssl s_client -connect reader.example.com:443 \
  -servername reader.example.com </dev/null 2>/dev/null | openssl x509 -noout -dates
```

Do not enable both profiles: they intentionally compete for ports 80 and 443.

---

## D. Proxmox

On Proxmox VE you first create a small Linux machine, then run Rusty Feed inside it
with Docker Compose (Option A). A container (LXC) is lightweight; a VM is the most
compatible. Below is the LXC route.

### 1. Create a Debian container (LXC)
1. In the Proxmox web UI: **Create CT**.
2. Use a **Debian 12** template, give it ~2 vCPU, 2–4 GB RAM, 20+ GB disk.
3. Start the container and open its console (or SSH in).

> **Docker inside LXC:** edit the container options so Docker works — enable
> **Nesting** (Options → Features → *nesting=1*), and *keyctl=1*. If you prefer
> to avoid this, create a **VM** instead and install Debian/Ubuntu normally; then
> the steps are identical from step 2.

### 2. Install Docker (inside the container/VM)

```bash
apt update && apt install -y curl git
curl -fsSL https://get.docker.com | sh
docker --version && docker compose version
```

### 3. Deploy Rusty Feed
Follow **[Option A](#a-localhost-with-docker-compose-easiest)** from step 2
(clone, create `.env` with your secrets, then run the documented Compose command
with `--env-file .env`).

### 4. Reach it on your network
- The app listens on port **8080** inside the container.
- Use the container's IP: `http://<container-ip>:8080/api/health`.
- For a clean domain + HTTPS, put a reverse proxy (Caddy, Nginx Proxy Manager,
  or Traefik) in front and forward it to `<container-ip>:8080`, then set
  `BASE_URL` accordingly.

---

## Configuration essentials

Everything is configured with environment variables (see `.env.example` for the
full list). The ones you're most likely to touch:

| Variable | What it does | Default |
|---|---|---|
| `SESSION_SECRET` | Required instance secret (reserved for session-key rotation; current sessions are opaque DB-backed tokens) | — |
| `ENCRYPTION_SECRET` | Required master secret for encrypted stored-secret paths | — |
| `DATABASE_URL` | PostgreSQL 17 connection string (SQLite is not supported) | set by compose |
| `REDIS_URL` | Optional Redis L2 cache connection string | `redis://localhost:6379` |
| `CACHE_ENABLED` | Enable Redis L2; set `false` for in-process cache only | `true` |
| `CACHE_NAMESPACE` | Isolate keys and cross-instance invalidations; make unique when installations share Redis | `rusty-feed` |
| `POSTGRES_PORT` | PostgreSQL port published on the host | `5432` |
| `REDIS_PORT` | Redis port published on the host | `6379` |
| `CHROMIUM_PORT` | Chromium CDP port published on the host | `9222` |
| `BASE_URL` | Public URL of your instance (for correct links) | `http://<bind_addr>` |
| `REGISTRATION_MODE` | `open` / `closed` (`invite` is reserved) | `open` |
| `EMBEDDING_PROVIDER` | `local` (deterministic development placeholder) or `openai` | `local` |
| `OPENAI_API_KEY` | Needed only if using OpenAI for AI features | — |
| `LOG_LEVEL` | Log verbosity (`info`, `debug`, …) | `info` |

The app **validates configuration on startup** and refuses to boot if a required
value is missing or a secret is too short — the error message tells you what to
fix.

---

## Troubleshooting

- **Compose says a secret must be set** → you didn't set `SESSION_SECRET` /
  `ENCRYPTION_SECRET`. Put them in `.env` (see the secrets section).
- **`invalid configuration: SESSION_SECRET must be at least 32 bytes`** → your
  secret is too short; regenerate with `openssl rand -base64 48`.
- **Port already allocated** → set `POSTGRES_PORT` / `REDIS_PORT` /
  `CHROMIUM_PORT` in `.env` and include `--env-file .env` in the Compose command
  (see the note in Option A).
- **App can't reach the database** → confirm Postgres is healthy
  (`docker compose --env-file .env -f docker/compose.yaml ps`). The Compose app
  uses `postgres:5432` internally; Rusty Feed and database tools running directly on
  your machine use `localhost:$POSTGRES_PORT` via `DATABASE_URL`.
- **First build is slow** → the Rust app compiles on first build; subsequent
  builds are cached and much faster.
- **See the logs** → `docker compose --env-file .env -f docker/compose.yaml logs -f app`.

Still stuck? Open an issue on the repository.

---

## What next?

- Read the [README](../README.md) for the project vision and tech stack.
- Curious how it works? See the [Architecture](ARCHITECTURE.md) doc.
- Want to contribute? Start with [AGENTS.md](../AGENTS.md) and the
  [Development Plan](DEVELOPMENT_PLAN.md).
