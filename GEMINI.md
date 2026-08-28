# GEMINI.md

Guidance for Gemini (and other AI assistants) working in this repository.

**➡️ The canonical guide is [`AGENTS.md`](./AGENTS.md). Read it first.** This file is a pointer.

**Project:** RustSS — a self-hosted, multi-user, AI-augmented RSS reader in Rust
(Axum + Askama/HTMX, PostgreSQL/pgvector, Redis, background "AGENTS" workers). A
ground-up Rust reimplementation of the original `neureed` (Next.js/TypeScript).

**The one rule — never break it:** *Content is shared; state is personal.*
Feeds/articles/embeddings/summaries are stored and computed **once** and shared
across subscribers; per-user data layers on top. A feed followed by *N* users must
be O(1) work, not O(N). (`docs/ARCHITECTURE.md` §7, `docs/PRD.md` §6.)

**Read before coding:** `docs/PRD.md`, `docs/ARCHITECTURE.md`,
`docs/DEVELOPMENT_PLAN.md`, `docs/FINDINGS.md`. Work the current milestone in the
development plan. Log new smells/bugs/risks in `docs/FINDINGS.md`.

**Before every commit:** `cargo fmt --all --check` · `cargo clippy --all-targets
--all-features -- -D warnings` · `cargo test`. Conventional Commits with a **why**
in the body. Don't push unless asked. Never weaken security controls to pass CI.

See `AGENTS.md` §5–§8 for the full conventions, Definition of Done, and gotchas.
