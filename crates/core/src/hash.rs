//! Content hashing used for article deduplication.
//!
//! The hash is `SHA-256(trim(content))` as a lowercase hex string, matching the
//! reference implementation so behavior is comparable (`docs/ARCHITECTURE.md`
//! §11.1). It is a pure function with no I/O, deliberately kept in `core` so
//! both ingest (`rustss-feeds`) and the repositories (`rustss-db`) agree on it.

use sha2::{Digest, Sha256};

/// Compute the content hash for an article body: `SHA-256(trim(content))` hex.
pub fn content_hash(content: &str) -> String {
    sha256_hex(content.trim().as_bytes())
}

/// Lowercase-hex SHA-256 of arbitrary bytes. Used for content hashing and for
/// hashing opaque secrets (session/API tokens) before storage.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex_encode(&hasher.finalize())
}

/// Encode bytes as a lowercase hex string (e.g. for random opaque tokens).
pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_and_trims() {
        // Known SHA-256 of "hello".
        let expected = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert_eq!(content_hash("hello"), expected);
        // Leading/trailing whitespace is ignored.
        assert_eq!(content_hash("  hello\n"), expected);
    }

    #[test]
    fn different_content_differs() {
        assert_ne!(content_hash("a"), content_hash("b"));
    }
}
