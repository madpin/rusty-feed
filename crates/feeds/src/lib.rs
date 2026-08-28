//! # rustss-feeds
//!
//! Feed fetching and ingest: the SSRF-guarded `SafeFetchClient`, `feed-rs`
//! parsing (RSS/Atom/JSON Feed), conditional GET (ETag/Last-Modified), and
//! article deduplication (GUID -> URL -> content hash).
//!
//! Implemented in milestone **M6** (see `docs/ARCHITECTURE.md` §11.1, §12).
