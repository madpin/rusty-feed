# Getting Started with RustSS

This guide walks you through installing and running **RustSS** — step by step, no
Rust knowledge required. Pick the option that matches where you want to run it:

- [A. Localhost with Docker Compose](#a-localhost-with-docker-compose-easiest) — easiest, recommended
- [B. Localhost from source](#b-localhost-from-source-for-developers) — for developers / contributors
- [C. Dokploy](#c-dokploy) — one-click-ish self-hosted PaaS
- [D. Proxmox](#d-proxmox) — home-lab / self-hosted server

> **Current status (early development).** RustSS is being built milestone by
> milestone. Right now the app boots, connects to its database, applies its
> schema, and serves health/metrics endpoints (`/api/health`, `/metrics`). The
> full reading web UI is still under construction. These install steps are
> correct and will keep working as features land — you'll just see more of the
> app over time. If you only see a health JSON response for now, that's expected.

---

## Before you start: generate your secrets

RustSS needs two secret values (at least 32 characters each). You'll paste these
into a configuration file in the steps below. Generate them now:

```bash
# Run each command and copy the output somewhere safe.
openssl rand -base64 48   # use this for SESSION_SECRET
openssl rand -base64 48   # use this for ENCRYPTION_SECRET
```

- **SESSION_SECRET** — signs login sessions.
- **ENCRYPTION_SECRET** — encrypts stored API keys and feed cookies. **If you
  lose it, those stored secrets can't be decrypted**, so keep it safe and back it
  up. Don't change it after first run unless you know what you're doing.

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
git clone https://github.com/madpin/rustss.git
cd rustss
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
docker compose -f docker/compose.yaml up -d
```

The **first** run builds the app and downloads images, so it can take several
minutes. Later starts are fast.

> **Port already in use?** If you see an error about port `5432`, `6379`, `9222`,
> or `8080` being allocated, something else on your machine uses it. Set an
> alternative in your `.env`, for example:
> ```bash
> POSTGRES_PORT=55432
> REDIS_PORT=56379
> CHROMIUM_PORT=59222
> ```
> (The app itself stays on `8080`; change the left side of `"8080:8080"` in
> `docker/compose.yaml` if you need a different app port.)

### 5. Check it's running

```bash
curl http://localhost:8080/api/health
# {"data":{"service":"RustSS","status":"ok","version":"0.1.0"}}
```

Open <http://localhost:8080/api/health> in your browser too.

### 6. Everyday commands

```bash
docker compose -f docker/compose.yaml logs -f app     # watch app logs
docker compose -f docker/compose.yaml ps              # what's running
docker compose -f docker/compose.yaml down            # stop (keeps data)
docker compose -f docker/compose.yaml up -d --build   # rebuild after updating code
docker compose -f docker/compose.yaml down -v         # stop AND delete all data ⚠️
```

Database migrations run automatically on startup (`AUTO_MIGRATE=true`).

---

## B. Localhost from source (for developers)

Use this if you want to change the code. You'll run the app with Rust's build
tool, but still use Docker for the database, Redis, and Chrome (much easier than
installing them by hand).

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
git clone https://github.com/madpin/rustss.git
cd rustss
cp .env.example .env       # then edit .env and set SESSION_SECRET + ENCRYPTION_SECRET
```

### 3. Start the dependencies (database, Redis, Chrome)

```bash
docker compose -f docker/compose.yaml up -d postgres redis chromium
```

Make sure your `.env` `DATABASE_URL` points at this Postgres. With the default
compose port that's:

```bash
DATABASE_URL=postgres://rustss:rustss@localhost:5432/rustss
REDIS_URL=redis://localhost:6379
```

(If you changed `POSTGRES_PORT`, update the port in `DATABASE_URL` to match.)

### 4. Create the database schema

```bash
cargo run -p rustss-cli -- migrate
# -> "migrations applied"
```

### 5. Run the app

```bash
cargo run -p rustss-server
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
cargo run -p rustss-worker                           # run background workers (once implemented)
```

<details>
<summary>Running the database/Redis natively (without Docker)</summary>

You can install PostgreSQL 16 with the **pgvector** extension and Redis directly
on your machine instead of using Docker, then point `DATABASE_URL` /`REDIS_URL`
at them. pgvector must be installed and the database must allow
`CREATE EXTENSION vector` (RustSS's migration does this for you). Docker is
recommended because it bundles the correct Postgres + pgvector version for you.
</details>

---

## C. Dokploy

[Dokploy](https://dokploy.com) is a self-hosted platform that deploys Docker apps
from a Git repo. RustSS ships a Compose file it can use directly.

### 1. Prepare
- Have Dokploy installed on your server (see Dokploy's own install docs).
- Push your copy of RustSS to a Git repository Dokploy can reach (GitHub, GitLab,
  or a private repo with credentials).

### 2. Create a Compose service
1. In the Dokploy dashboard, create a **Project**, then add a **Compose** service.
2. Point it at your RustSS Git repository and branch.
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

## D. Proxmox

On Proxmox VE you first create a small Linux machine, then run RustSS inside it
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

### 3. Deploy RustSS
Follow **[Option A](#a-localhost-with-docker-compose-easiest)** from step 2
(clone, create `.env` with your secrets, `docker compose up -d`).

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
| `SESSION_SECRET` | Signs login sessions (**required**, 32+ chars) | — |
| `ENCRYPTION_SECRET` | Encrypts stored API keys/cookies (**required**, 32+ chars) | — |
| `DATABASE_URL` | PostgreSQL connection string | set by compose |
| `REDIS_URL` | Redis connection string | `redis://localhost:6379` |
| `BASE_URL` | Public URL of your instance (for correct links) | `http://<bind_addr>` |
| `REGISTRATION_MODE` | `open` / `invite` / `closed` | `open` |
| `EMBEDDING_PROVIDER` | `local` (free, on-device) or `openai` | `local` |
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
  `CHROMIUM_PORT` in `.env` (see the note in Option A).
- **App can't reach the database** → confirm Postgres is healthy
  (`docker compose -f docker/compose.yaml ps`) and `DATABASE_URL` matches its
  host/port. From the app container the host is `postgres`; from your machine it's
  `localhost`.
- **First build is slow** → the Rust app compiles on first build; subsequent
  builds are cached and much faster.
- **See the logs** → `docker compose -f docker/compose.yaml logs -f app`.

Still stuck? Open an issue on the repository.

---

## What next?

- Read the [README](../README.md) for the project vision and tech stack.
- Curious how it works? See the [Architecture](ARCHITECTURE.md) doc.
- Want to contribute? Start with [AGENTS.md](../AGENTS.md) and the
  [Development Plan](DEVELOPMENT_PLAN.md).
