//! Ensure Cargo recompiles this crate (re-embedding `sqlx::migrate!`) whenever
//! the migrations change.
//!
//! On stable Rust, `sqlx::migrate!("./migrations")` cannot tell Cargo about the
//! files it embeds, so a cached build (e.g. Docker's `target` cache) can ship a
//! binary with a **stale** migration set — leading to sqlx errors like
//! "migration N was previously applied but is missing in the resolved
//! migrations". Emitting `rerun-if-changed` for the directory (Cargo scans it
//! recursively) forces a rebuild on any add/edit/remove.

fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
