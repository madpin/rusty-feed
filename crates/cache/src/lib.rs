//! # rustss-cache
//!
//! Two-tier cache behind the `Cache` port: in-process `moka` (L1) plus optional
//! Redis (L2) with cross-instance invalidation. Shared vs. per-user key
//! separation maximizes cross-user reuse.
//!
//! Implemented in milestone **M3** (see `docs/ARCHITECTURE.md` §10).
