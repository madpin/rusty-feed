//! Authenticated encryption for secrets at rest (M12; resolves FND-004).
//!
//! One unified scheme — **XChaCha20-Poly1305** with a key derived from
//! `ENCRYPTION_SECRET` via **HKDF-SHA256** — used for every stored secret (API
//! keys, feed cookies). The ciphertext envelope is `base64(version ‖ nonce ‖
//! ct)`, and the version byte is authenticated as associated data, so a stored
//! blob can be re-keyed later by adding a new version (FND-110).

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;

/// Current key version (authenticated in the envelope).
const KEY_VERSION: u8 = 1;
const NONCE_LEN: usize = 24; // XChaCha20 nonce
const HKDF_INFO: &[u8] = b"rustss/aead/v1";

/// Errors from the crypto layer.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("invalid ciphertext envelope")]
    Envelope,
    #[error("unsupported key version {0}")]
    Version(u8),
    #[error("decryption failed (wrong key or tampered data)")]
    Decrypt,
}

/// Encrypts/decrypts secrets with a key derived from the instance secret.
#[derive(Clone)]
pub struct Cryptor {
    cipher: XChaCha20Poly1305,
}

impl Cryptor {
    /// Derive the encryption key from the raw `ENCRYPTION_SECRET`.
    pub fn new(encryption_secret: &[u8]) -> Self {
        let hk = Hkdf::<Sha256>::new(None, encryption_secret);
        let mut key = [0u8; 32];
        hk.expand(HKDF_INFO, &mut key)
            .expect("HKDF expand into 32 bytes is valid");
        Self {
            cipher: XChaCha20Poly1305::new((&key).into()),
        }
    }

    /// Encrypt bytes → `base64(version ‖ nonce ‖ ciphertext)`.
    pub fn encrypt(&self, plaintext: &[u8]) -> String {
        let mut nonce = [0u8; NONCE_LEN];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let aad = [KEY_VERSION];
        let ciphertext = self
            .cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .expect("XChaCha20-Poly1305 encryption does not fail");

        let mut envelope = Vec::with_capacity(1 + NONCE_LEN + ciphertext.len());
        envelope.push(KEY_VERSION);
        envelope.extend_from_slice(&nonce);
        envelope.extend_from_slice(&ciphertext);
        BASE64.encode(envelope)
    }

    /// Encrypt a UTF-8 string.
    pub fn encrypt_str(&self, plaintext: &str) -> String {
        self.encrypt(plaintext.as_bytes())
    }

    /// Decrypt a `base64(version ‖ nonce ‖ ciphertext)` envelope.
    pub fn decrypt(&self, encoded: &str) -> Result<Vec<u8>, CryptoError> {
        let data = BASE64.decode(encoded).map_err(|_| CryptoError::Envelope)?;
        if data.len() < 1 + NONCE_LEN + 16 {
            return Err(CryptoError::Envelope);
        }
        let version = data[0];
        if version != KEY_VERSION {
            return Err(CryptoError::Version(version));
        }
        let nonce = &data[1..1 + NONCE_LEN];
        let ciphertext = &data[1 + NONCE_LEN..];
        let aad = [version];
        self.cipher
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| CryptoError::Decrypt)
    }

    /// Decrypt to a UTF-8 string.
    pub fn decrypt_str(&self, encoded: &str) -> Result<String, CryptoError> {
        let bytes = self.decrypt(encoded)?;
        String::from_utf8(bytes).map_err(|_| CryptoError::Decrypt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let c = Cryptor::new(b"super-secret-master-key-material");
        let enc = c.encrypt_str("sk-my-api-key");
        assert_ne!(enc, "sk-my-api-key");
        assert_eq!(c.decrypt_str(&enc).unwrap(), "sk-my-api-key");
    }

    #[test]
    fn nonce_makes_ciphertexts_differ() {
        let c = Cryptor::new(b"master");
        assert_ne!(c.encrypt_str("same"), c.encrypt_str("same"));
    }

    #[test]
    fn tampering_is_detected() {
        let c = Cryptor::new(b"master");
        let enc = c.encrypt_str("secret");
        let mut raw = BASE64.decode(&enc).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0x01; // flip a ciphertext bit
        let tampered = BASE64.encode(raw);
        assert!(matches!(c.decrypt(&tampered), Err(CryptoError::Decrypt)));
    }

    #[test]
    fn wrong_key_fails() {
        let a = Cryptor::new(b"key-a");
        let b = Cryptor::new(b"key-b");
        let enc = a.encrypt_str("secret");
        assert!(b.decrypt(&enc).is_err());
    }

    #[test]
    fn garbage_is_rejected() {
        let c = Cryptor::new(b"master");
        assert!(matches!(
            c.decrypt("not base64!!!"),
            Err(CryptoError::Envelope)
        ));
        assert!(matches!(c.decrypt("YWJj"), Err(CryptoError::Envelope))); // too short
    }
}
