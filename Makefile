SHELL := /bin/bash
.DEFAULT_GOAL := help

CARGO ?= cargo
DOCKER ?= docker
NPM ?= npm
NPX ?= npx
CURL ?= curl
ENV_FILE ?= .env
COMPOSE_FILE ?= docker/compose.yaml
TAIL ?= 200
ARGS ?=
TEST_ARGS ?= --nocapture
IMAGE ?= rusty-feed:dev
DB_USER ?= rusty_feed
DB_NAME ?=
TEST_DB ?= rusty_feed_test
E2E_DB ?= rusty_feed_e2e
STRESS_DB ?= rusty_feed_stress
CONFIRM ?= 0

get_env_port = $(strip $(shell if [ -f "$(ENV_FILE)" ]; then awk -F= -v key="$(1)" '$$1 == key { print $$2; exit }' "$(ENV_FILE)" | cut -d '\#' -f 1 | tr -d '[:space:]'; fi))
ENV_POSTGRES_PORT := $(call get_env_port,POSTGRES_PORT)
ENV_REDIS_PORT := $(call get_env_port,REDIS_PORT)
ENV_APP_PORT := $(call get_env_port,APP_PORT)
POSTGRES_PORT ?= $(if $(ENV_POSTGRES_PORT),$(ENV_POSTGRES_PORT),5432)
REDIS_PORT ?= $(if $(ENV_REDIS_PORT),$(ENV_REDIS_PORT),6379)
APP_PORT ?= $(if $(ENV_APP_PORT),$(ENV_APP_PORT),8080)

TEST_DATABASE_URL ?= postgres://rusty_feed:rusty_feed@127.0.0.1:$(POSTGRES_PORT)/$(TEST_DB)
E2E_DATABASE_URL ?= postgres://rusty_feed:rusty_feed@127.0.0.1:$(POSTGRES_PORT)/$(E2E_DB)
STRESS_DATABASE_URL ?= postgres://rusty_feed:rusty_feed@127.0.0.1:$(POSTGRES_PORT)/$(STRESS_DB)
TEST_REDIS_URL ?= redis://127.0.0.1:$(REDIS_PORT)
APP_URL ?= http://127.0.0.1:$(APP_PORT)

COMPOSE = $(DOCKER) compose --env-file "$(ENV_FILE)" -f "$(COMPOSE_FILE)"
CLI = $(CARGO) run --quiet -p rusty-feed-cli --
SERVER = $(CARGO) run -p rusty-feed-server --
WORKER = $(CARGO) run -p rusty-feed-worker --

.PHONY: help list doctor versions setup env-init require-env \
	dev dev-minimal run run-worker debug debug-worker watch watch-test browse health ready metrics \
	build build-release check fmt fmt-check lint test test-crate test-one test-nocapture test-db \
	test-e2e test-e2e-headed test-e2e-ui test-stress test-stress-enforced test-tls test-all \
	quick verify ci docs docs-open deps-tree audit clean metadata \
	npm-install playwright-install \
	docker-config docker-build docker-image docker-up docker-app docker-deps docker-test-deps \
	docker-rebuild docker-pull docker-stop docker-down docker-reset docker-ps docker-logs \
	docker-logs-app docker-logs-postgres docker-logs-redis docker-shell docker-exec \
	docker-tls-auto docker-tls-custom \
	db-up db-create db-test-prepare db-e2e-prepare db-stress-prepare db-shell db-status db-drop \
	redis-shell migrate config-check create-admin fetch-model embeddings-backfill urls-check \
	urls-apply embeddings-reindex demo-reset cli-help version version-bump

##@ Getting started
help: ## Show this command guide
	@printf "Rusty Feed development commands\n\n"
	@printf "Usage: make <target> [VARIABLE=value]\n"
	@printf "Example: make test-one TEST=dedup TEST_ARGS='--nocapture'\n"
	@awk 'BEGIN { printf "\n" } /^##@/ { printf "%s\n", substr($$0, 5); next } /^[A-Za-z0-9_.-]+:([^=]|$$)/ { if (match($$0, /## .*/)) { target=$$1; sub(/:.*/, "", target); description=substr($$0, RSTART + 3); printf "  %-24s %s\n", target, description } }' $(MAKEFILE_LIST)
	@printf "\nCommon variables: ENV_FILE, ARGS, CRATE, TEST, TEST_ARGS, DB_NAME, CMD, IMAGE, VERSION, TAIL, CONFIRM\n"

list: help ## Alias for help

doctor: ## Check required development tools and Docker
	@missing=0; \
	for tool in $(CARGO) rustc rustfmt $(DOCKER) node $(NPM) $(NPX) openssl $(CURL); do \
		if command -v "$$tool" >/dev/null 2>&1; then printf "[ok]      %s\n" "$$tool"; else printf "[missing] %s\n" "$$tool"; missing=1; fi; \
	done; \
	if $(DOCKER) compose version >/dev/null 2>&1; then printf "[ok]      docker compose\n"; else printf "[missing] docker compose\n"; missing=1; fi; \
	if $(DOCKER) info >/dev/null 2>&1; then printf "[ok]      Docker daemon\n"; else printf "[missing] Docker daemon is not reachable\n"; missing=1; fi; \
	exit $$missing

versions: ## Print development tool versions
	@$(CARGO) --version
	@rustc --version
	@rustfmt --version
	@$(DOCKER) --version
	@$(DOCKER) compose version
	@node --version
	@$(NPM) --version
	@$(NPX) playwright --version 2>/dev/null || printf "Playwright is not installed; run make npm-install\n"

setup: ## Prepare env, containers, databases, Node, and migrations
	@$(MAKE) --no-print-directory doctor
	@$(MAKE) --no-print-directory env-init
	@$(MAKE) --no-print-directory docker-deps
	@$(MAKE) --no-print-directory db-test-prepare
	@$(MAKE) --no-print-directory db-e2e-prepare
	@$(MAKE) --no-print-directory db-stress-prepare
	@$(MAKE) --no-print-directory npm-install
	@$(MAKE) --no-print-directory playwright-install
	@$(MAKE) --no-print-directory migrate
	@printf "\nSetup complete. Start Rusty Feed with: make dev\n"

env-init: ## Create .env with generated development secrets
	@if [ -f "$(ENV_FILE)" ]; then \
		printf "%s already exists; leaving it unchanged\n" "$(ENV_FILE)"; \
	else \
		command -v openssl >/dev/null 2>&1 || { printf "openssl is required to generate secrets\n" >&2; exit 1; }; \
		tmp="$$(mktemp)"; \
		session="$$(openssl rand -base64 48)"; \
		encryption="$$(openssl rand -base64 48)"; \
		awk -v session="$$session" -v encryption="$$encryption" ' \
			/^SESSION_SECRET=change-me-32-bytes-min$$/ { print "SESSION_SECRET=" session; next } \
			/^ENCRYPTION_SECRET=change-me-32-bytes-min$$/ { print "ENCRYPTION_SECRET=" encryption; next } \
			{ print } \
		' .env.example > "$$tmp"; \
		mv "$$tmp" "$(ENV_FILE)"; \
		chmod 600 "$(ENV_FILE)"; \
		printf "Created %s with generated development secrets\n" "$(ENV_FILE)"; \
	fi

require-env:
	@test -f "$(ENV_FILE)" || { printf "%s is missing; run make env-init\n" "$(ENV_FILE)" >&2; exit 1; }

##@ Run and debug
dev: ## Start dependencies, migrate, and run the server
	@$(MAKE) --no-print-directory docker-deps
	@$(MAKE) --no-print-directory migrate
	@$(MAKE) --no-print-directory run

dev-minimal: ## Run with PostgreSQL only, without Redis or Chromium
	@$(MAKE) --no-print-directory db-up
	@$(MAKE) --no-print-directory migrate
	@CACHE_ENABLED=false CHROMIUM_ENABLED=false $(SERVER) $(ARGS)

run: require-env ## Run server with the embedded worker
	@$(SERVER) $(ARGS)

run-worker: require-env ## Run the standalone AGENTS worker
	@$(WORKER) $(ARGS)

debug: require-env ## Run server with debug logs and full backtraces
	@LOG_LEVEL=debug RUST_BACKTRACE=full $(SERVER) $(ARGS)

debug-worker: require-env ## Run worker with debug logs and full backtraces
	@LOG_LEVEL=debug RUST_BACKTRACE=full $(WORKER) $(ARGS)

watch: require-env ## Rebuild and run server on Rust changes (cargo-watch)
	@command -v cargo-watch >/dev/null 2>&1 || { printf "cargo-watch is missing; install it with: cargo install cargo-watch --locked\n" >&2; exit 1; }
	@cargo watch -x 'run -p rusty-feed-server'

watch-test: ## Re-run tests on Rust changes (cargo-watch)
	@command -v cargo-watch >/dev/null 2>&1 || { printf "cargo-watch is missing; install it with: cargo install cargo-watch --locked\n" >&2; exit 1; }
	@cargo watch -x 'test --workspace --all-features'

browse: ## Open the local application in a browser
	@if command -v open >/dev/null 2>&1; then open "$(APP_URL)"; elif command -v xdg-open >/dev/null 2>&1; then xdg-open "$(APP_URL)"; else printf "%s\n" "$(APP_URL)"; fi

health: ## Query the application health endpoint
	@$(CURL) --fail --show-error --silent "$(APP_URL)/api/health"; printf "\n"

ready: ## Query the application readiness endpoint
	@$(CURL) --fail --show-error --silent "$(APP_URL)/api/health/ready"; printf "\n"

metrics: ## Print Prometheus metrics
	@$(CURL) --fail --show-error --silent "$(APP_URL)/metrics"

##@ Rust
build: ## Build the workspace
	@$(CARGO) build --workspace

build-release: ## Build the optimized workspace
	@$(CARGO) build --release --workspace

check: ## Type-check all targets and features
	@$(CARGO) check --workspace --all-targets --all-features

fmt: ## Format all Rust code
	@$(CARGO) fmt --all

fmt-check: ## Verify Rust formatting
	@$(CARGO) fmt --all --check

lint: ## Run Clippy and deny warnings
	@$(CARGO) clippy --all-targets --all-features -- -D warnings

test: ## Run workspace tests; external-service tests may skip
	@$(CARGO) test --workspace --all-features

test-crate: ## Test one crate (CRATE=rusty-feed-web)
	@test -n "$(CRATE)" || { printf "CRATE is required, e.g. make test-crate CRATE=rusty-feed-web\n" >&2; exit 2; }
	@$(CARGO) test -p "$(CRATE)" --all-features -- $(TEST_ARGS)

test-one: ## Run matching tests (TEST=name)
	@test -n "$(TEST)" || { printf "TEST is required, e.g. make test-one TEST=dedup\n" >&2; exit 2; }
	@$(CARGO) test --workspace --all-features "$(TEST)" -- $(TEST_ARGS)

test-nocapture: ## Run workspace tests with output and backtraces
	@RUST_BACKTRACE=1 $(CARGO) test --workspace --all-features -- --nocapture

test-db: ## Run all tests with dedicated PostgreSQL and Redis
	@$(MAKE) --no-print-directory docker-test-deps
	@$(MAKE) --no-print-directory db-test-prepare
	@TEST_DATABASE_URL="$(TEST_DATABASE_URL)" TEST_REDIS_URL="$(TEST_REDIS_URL)" RUST_BACKTRACE=1 $(CARGO) test --workspace --all-features

test-e2e: node_modules/.package-lock.json ## Run headless Playwright tests
	@$(MAKE) --no-print-directory docker-test-deps
	@$(MAKE) --no-print-directory db-e2e-prepare
	@PLAYWRIGHT_DATABASE_URL="$(E2E_DATABASE_URL)" PLAYWRIGHT_REDIS_URL="$(TEST_REDIS_URL)" $(NPM) run test:e2e

test-e2e-headed: node_modules/.package-lock.json ## Run Playwright in a headed browser
	@$(MAKE) --no-print-directory docker-test-deps
	@$(MAKE) --no-print-directory db-e2e-prepare
	@PLAYWRIGHT_DATABASE_URL="$(E2E_DATABASE_URL)" PLAYWRIGHT_REDIS_URL="$(TEST_REDIS_URL)" $(NPM) run test:e2e:headed

test-e2e-ui: node_modules/.package-lock.json ## Open Playwright's interactive test UI
	@$(MAKE) --no-print-directory docker-test-deps
	@$(MAKE) --no-print-directory db-e2e-prepare
	@PLAYWRIGHT_DATABASE_URL="$(E2E_DATABASE_URL)" PLAYWRIGHT_REDIS_URL="$(TEST_REDIS_URL)" $(NPM) run test:e2e:ui

test-stress: node_modules/.package-lock.json ## Run the opt-in full-stack stress suite
	@$(MAKE) --no-print-directory docker-test-deps
	@$(MAKE) --no-print-directory db-stress-prepare
	@TEST_DATABASE_URL="$(STRESS_DATABASE_URL)" TEST_REDIS_URL="$(TEST_REDIS_URL)" $(NPM) run test:stress

test-stress-enforced: node_modules/.package-lock.json ## Run stress tests and enforce performance targets
	@$(MAKE) --no-print-directory docker-test-deps
	@$(MAKE) --no-print-directory db-stress-prepare
	@STRESS_ENFORCE_TARGETS=1 TEST_DATABASE_URL="$(STRESS_DATABASE_URL)" TEST_REDIS_URL="$(TEST_REDIS_URL)" $(NPM) run test:stress

test-tls: ## Validate Caddy TLS profiles and HTTP/2
	@scripts/test-tls-proxy.sh

test-all: ## Run DB integration, browser, stress, and TLS suites
	@$(MAKE) --no-print-directory test-db
	@$(MAKE) --no-print-directory test-e2e
	@$(MAKE) --no-print-directory test-stress
	@$(MAKE) --no-print-directory test-tls

quick: ## Run fast formatting, type, and unit checks
	@$(MAKE) --no-print-directory fmt-check
	@$(MAKE) --no-print-directory check
	@$(MAKE) --no-print-directory test

verify: ## Run the standard definition-of-done Rust checks
	@$(MAKE) --no-print-directory fmt-check
	@$(MAKE) --no-print-directory lint
	@$(MAKE) --no-print-directory test-db

ci: ## Reproduce the local CI quality gates
	@$(MAKE) --no-print-directory verify
	@$(MAKE) --no-print-directory npm-install
	@$(MAKE) --no-print-directory playwright-install
	@$(MAKE) --no-print-directory test-e2e
	@$(MAKE) --no-print-directory test-tls
	@$(MAKE) --no-print-directory build-release
	@$(MAKE) --no-print-directory docker-config

docs: ## Build Rust documentation for all features
	@$(CARGO) doc --workspace --all-features --no-deps

docs-open: ## Build and open Rust documentation
	@$(CARGO) doc --workspace --all-features --no-deps --open

deps-tree: ## Print the Cargo dependency tree
	@$(CARGO) tree --workspace --all-features

audit: ## Run cargo-audit when installed
	@command -v cargo-audit >/dev/null 2>&1 || { printf "cargo-audit is missing; install it with: cargo install cargo-audit --locked\n" >&2; exit 1; }
	@$(CARGO) audit

clean: ## Remove Cargo build artifacts
	@$(CARGO) clean

metadata: ## Validate locked Cargo workspace metadata
	@$(CARGO) metadata --locked --no-deps --format-version 1 >/dev/null
	@printf "Cargo metadata is valid and Cargo.lock is current\n"

##@ Browser tooling
node_modules/.package-lock.json: package.json package-lock.json
	@$(NPM) ci

npm-install: ## Install locked Playwright dependencies
	@$(NPM) ci

playwright-install: node_modules/.package-lock.json ## Install Playwright Chromium
	@$(NPX) playwright install chromium

##@ Docker
docker-config: require-env ## Validate the rendered Compose configuration
	@$(COMPOSE) config --quiet

docker-build: require-env ## Build the Compose application image
	@$(COMPOSE) build app

docker-image: ## Build a standalone image (IMAGE=rusty-feed:dev)
	@$(DOCKER) build -f docker/Dockerfile -t "$(IMAGE)" .

docker-up: require-env ## Start the complete application stack
	@$(COMPOSE) up -d

docker-app: require-env ## Start the application and required database
	@$(COMPOSE) up -d app

docker-deps: require-env ## Start PostgreSQL, Redis, and Chromium
	@$(COMPOSE) up -d postgres redis chromium

docker-test-deps: require-env ## Start PostgreSQL and Redis for tests
	@$(COMPOSE) up -d postgres redis

docker-rebuild: require-env ## Rebuild and restart the complete stack
	@$(COMPOSE) up -d --build

docker-pull: require-env ## Pull service images
	@$(COMPOSE) pull

docker-stop: require-env ## Stop services without removing containers
	@$(COMPOSE) stop

docker-down: require-env ## Remove containers and networks; keep volumes
	@$(COMPOSE) down --remove-orphans

docker-reset: require-env ## Delete containers and all Compose volumes (CONFIRM=1)
	@test "$(CONFIRM)" = "1" || { printf "This deletes all Compose data. Rerun with CONFIRM=1\n" >&2; exit 2; }
	@$(COMPOSE) down --volumes --remove-orphans

docker-ps: require-env ## Show Compose service status
	@$(COMPOSE) ps

docker-logs: require-env ## Follow all logs (TAIL=200)
	@$(COMPOSE) logs --follow --tail="$(TAIL)"

docker-logs-app: require-env ## Follow application logs (TAIL=200)
	@$(COMPOSE) logs --follow --tail="$(TAIL)" app

docker-logs-postgres: require-env ## Follow PostgreSQL logs (TAIL=200)
	@$(COMPOSE) logs --follow --tail="$(TAIL)" postgres

docker-logs-redis: require-env ## Follow Redis logs (TAIL=200)
	@$(COMPOSE) logs --follow --tail="$(TAIL)" redis

docker-shell: require-env ## Open a shell in the application container
	@$(COMPOSE) exec app /bin/sh

docker-exec: require-env ## Run CMD in app (CMD='rusty-feed-cli help')
	@test -n "$(CMD)" || { printf "CMD is required, e.g. make docker-exec CMD='rusty-feed-cli help'\n" >&2; exit 2; }
	@$(COMPOSE) exec app $(CMD)

docker-tls-auto: require-env ## Start stack with automatic Caddy TLS
	@$(COMPOSE) --profile https-auto up -d

docker-tls-custom: require-env ## Start stack with custom-certificate Caddy TLS
	@$(COMPOSE) --profile https-custom up -d

##@ Data services
db-up: require-env ## Start PostgreSQL
	@$(COMPOSE) up -d postgres

db-create: require-env ## Create a database when absent (DB_NAME=name)
	@test -n "$(DB_NAME)" || { printf "DB_NAME is required\n" >&2; exit 2; }
	@[[ "$(DB_NAME)" =~ ^[A-Za-z_][A-Za-z0-9_]*$$ ]] || { printf "DB_NAME must be a safe PostgreSQL identifier\n" >&2; exit 2; }
	@$(COMPOSE) up -d postgres
	@database="$(DB_NAME)"; \
	existing="$$($(COMPOSE) exec -T postgres psql --username "$(DB_USER)" --dbname postgres --tuples-only --no-align --command "SELECT 1 FROM pg_database WHERE datname = '$$database'" | tr -d '[:space:]')"; \
	if [ "$$existing" = "1" ]; then printf "Database %s already exists\n" "$$database"; else $(COMPOSE) exec -T postgres createdb --username "$(DB_USER)" "$$database"; printf "Created database %s\n" "$$database"; fi

db-test-prepare: ## Create the dedicated Rust integration-test database
	@$(MAKE) --no-print-directory db-create DB_NAME="$(TEST_DB)"

db-e2e-prepare: ## Create the dedicated Playwright database
	@$(MAKE) --no-print-directory db-create DB_NAME="$(E2E_DB)"

db-stress-prepare: ## Create the dedicated stress-test database
	@$(MAKE) --no-print-directory db-create DB_NAME="$(STRESS_DB)"

db-shell: require-env ## Open psql in the development database
	@$(COMPOSE) exec postgres psql --username "$(DB_USER)" --dbname rusty_feed

db-status: require-env ## Show PostgreSQL readiness and databases
	@$(COMPOSE) exec -T postgres pg_isready --username "$(DB_USER)" --dbname rusty_feed
	@$(COMPOSE) exec -T postgres psql --username "$(DB_USER)" --dbname postgres --command '\l'

db-drop: require-env ## Drop a non-primary database (DB_NAME=name CONFIRM=1)
	@test "$(CONFIRM)" = "1" || { printf "Dropping a database requires CONFIRM=1\n" >&2; exit 2; }
	@test -n "$(DB_NAME)" || { printf "DB_NAME is required\n" >&2; exit 2; }
	@[[ "$(DB_NAME)" =~ ^[A-Za-z_][A-Za-z0-9_]*$$ ]] || { printf "DB_NAME must be a safe PostgreSQL identifier\n" >&2; exit 2; }
	@test "$(DB_NAME)" != "rusty_feed" || { printf "Refusing to drop the primary rusty_feed database\n" >&2; exit 2; }
	@$(COMPOSE) exec -T postgres dropdb --if-exists --force --username "$(DB_USER)" "$(DB_NAME)"

redis-shell: require-env ## Open redis-cli
	@$(COMPOSE) exec redis redis-cli

##@ Maintenance CLI
migrate: require-env ## Apply database migrations
	@$(CLI) migrate

config-check: require-env ## Validate config and print a secret-safe summary
	@$(CLI) config-check

create-admin: require-env ## Create/promote admin from exported RUSTY_FEED_ADMIN_* vars
	@$(CLI) create-admin

fetch-model: require-env ## Download and verify the configured embedding model
	@$(CLI) fetch-model

embeddings-backfill: require-env ## Queue missing embeddings for the active model
	@$(CLI) backfill

urls-check: require-env ## Dry-run canonical URL cleanup
	@$(CLI) canonicalize-urls

urls-apply: require-env ## Apply collision-free URL cleanup (CONFIRM=1)
	@test "$(CONFIRM)" = "1" || { printf "Applying URL changes requires CONFIRM=1\n" >&2; exit 2; }
	@$(CLI) canonicalize-urls --apply

embeddings-reindex: require-env ## Replace vector space and reindex (CONFIRM=1)
	@test "$(CONFIRM)" = "1" || { printf "Reindexing replaces all stored vectors. Rerun with CONFIRM=1\n" >&2; exit 2; }
	@$(CLI) reindex --confirm

demo-reset: require-env ## Delete all demo accounts and their data (CONFIRM=1)
	@test "$(CONFIRM)" = "1" || { printf "Deleting demo accounts requires CONFIRM=1\n" >&2; exit 2; }
	@$(CLI) demo-reset --confirm

cli-help: ## Show the Rusty Feed maintenance CLI help
	@$(CLI) help

version: ## Print the Rusty Feed version
	@$(CLI) --version

version-bump: ## Run guarded release bump (VERSION=patch CONFIRM=1)
	@test "$(CONFIRM)" = "1" || { printf "Version bump requires CONFIRM=1 and disposable test databases\n" >&2; exit 2; }
	@test -n "$(VERSION)" || { printf "VERSION is required: major, minor, patch, or X.Y.Z\n" >&2; exit 2; }
	@TEST_DATABASE_URL="$(TEST_DATABASE_URL)" TEST_REDIS_URL="$(TEST_REDIS_URL)" PLAYWRIGHT_DATABASE_URL="$(E2E_DATABASE_URL)" PLAYWRIGHT_REDIS_URL="$(TEST_REDIS_URL)" RUSTY_FEED_BUMP_CONFIRM_DATABASES=1 scripts/bump-version.sh "$(VERSION)"
