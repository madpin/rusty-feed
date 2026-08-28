//! # rustss-db
//!
//! PostgreSQL + pgvector persistence via SQLx: migrations and the repositories
//! that implement the `rustss-core` port traits.
//!
//! Implemented in milestone **M2** (see `docs/DEVELOPMENT_PLAN.md`). This is the
//! home of the shared/per-user data model that enforces the dedup invariant
//! (`docs/ARCHITECTURE.md` §6, §7).
