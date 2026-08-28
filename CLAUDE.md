# CLAUDE.md

Guidance for Claude Code (and other AI assistants) working in this repository.

**➡️ The canonical guide is [`AGENTS.md`](./AGENTS.md). Read it first — this file only highlights the essentials.**

## What this is

**RustSS** — a self-hosted, multi-user, AI-augmented RSS reader in Rust (Axum + Askama/HTMX, PostgreSQL/pgvector, Redis, background "AGENTS" workers). It is a ground-up Rust reimplementation of the original `neureed` (Next.js/TypeScript).

## The one rule you must never break

**Content is shared; state is personal.** Feeds/articles/embeddings/summaries are stored and computed **once** and shared across subscribers; per-user data layers on top. A feed followed by *N* users must be O(1) work, not O(N). (`docs/ARCHITECTURE.md` §7, `docs/PRD.md` §6.)

## Read before coding

- `docs/PRD.md` — requirements & targets
- `docs/ARCHITECTURE.md` — stack, data model, AGENTS framework, algorithms/constants (§11 has the authoritative values)
- `docs/DEVELOPMENT_PLAN.md` — milestone plan; work the current milestone
- `docs/FINDINGS.md` — known smells/bugs/risks; **add to it** when you find something (reference the `FND-*` id in your commit)

## Essential commands

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test
docker compose --env-file .env -f docker/compose.yaml up -d postgres redis chromium
cargo run -p rustss-cli -- migrate
cargo run -p rustss-server
```

## Non-negotiables (see AGENTS.md §5 for the full list)

- `thiserror` in libs, `anyhow` in bins; no `unwrap()`/`expect()` on fallible paths.
- SQLx **runtime** queries (`query`/`query_as` + `FromRow`), not `query!` (hermetic builds; validated by integration tests); per-user queries scoped by `user_id`; shared writes idempotent (`ON CONFLICT`).
- All outbound HTTP via `SafeFetchClient` (SSRF-guarded). Secrets encrypted; never logged.
- One embedding model per instance; never pad/truncate vectors across dimensions.
- Recurring work is an `Agent` through the framework — no ad-hoc `tokio::spawn` loops.
- Conventional Commits with a **why** in the body. Don't push unless asked. Never weaken security to pass CI.

## Definition of Done

fmt clean · clippy `-D warnings` clean · tests green (incl. dedup invariants with `TEST_DATABASE_URL`) · config documented · findings logged.
