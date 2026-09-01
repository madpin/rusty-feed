//! Opaque token generation for sessions and API tokens.
//!
//! We generate a high-entropy random token, hand the **raw** value to the client
//! (cookie / API key), and store only its SHA-256 hash — so a database read
//! never exposes a live credential.

use rand::RngCore;

/// A freshly generated token: the raw value (give to the client) and its hash
/// (store in the database).
pub struct GeneratedToken {
    pub raw: String,
    pub hash: String,
}

/// Generate a 256-bit random token (hex-encoded) plus its SHA-256 hash.
pub fn generate() -> GeneratedToken {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let raw = rusty_feed_core::hash::hex_encode(&bytes);
    let hash = rusty_feed_core::hash::sha256_hex(raw.as_bytes());
    GeneratedToken { raw, hash }
}

/// Hash a raw token the same way [`generate`] does (for lookups).
pub fn hash(raw: &str) -> String {
    rusty_feed_core::hash::sha256_hex(raw.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_hash_matches_hash_of_raw() {
        let t = generate();
        assert_eq!(t.hash, hash(&t.raw));
        assert_eq!(t.raw.len(), 64, "32 bytes hex-encoded");
    }

    #[test]
    fn tokens_are_unique() {
        assert_ne!(generate().raw, generate().raw);
    }
}
