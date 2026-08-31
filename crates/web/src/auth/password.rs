//! Password hashing and verification using Argon2id.

use argon2::password_hash::{
    rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::Argon2;

/// Hash a plaintext password with Argon2id (random salt). Returns the PHC string
/// suitable for storage in `users.password_hash`.
pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default().hash_password(password.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

/// Verify a plaintext password against a stored PHC hash. Returns `false` on
/// mismatch and on any parse error (never leaks the reason).
pub fn verify_password(password: &str, phc_hash: &str) -> bool {
    match PasswordHash::new(phc_hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// A small embedded denylist of the most common breached passwords. Kept short
/// and offline (no network dependency); public operators wanting deeper checks
/// can layer a k-anonymity range API in front (`docs/PUBLIC_LAUNCH_PLAN.md`
/// §4.5.4). Compared case-insensitively.
const COMMON_BREACHED_PASSWORDS: &[&str] = &[
    "password",
    "password1",
    "password123",
    "123456",
    "12345678",
    "123456789",
    "1234567890",
    "qwerty",
    "qwertyuiop",
    "qwerty123",
    "111111",
    "123123",
    "abc123",
    "letmein",
    "welcome",
    "welcome1",
    "admin",
    "administrator",
    "iloveyou",
    "monkey",
    "dragon",
    "sunshine",
    "princess",
    "football",
    "baseball",
    "master",
    "login",
    "passw0rd",
    "p@ssw0rd",
    "changeme",
    "trustno1",
    "whatever",
    "superman",
    "batman",
    "1q2w3e4r",
    "zaq12wsx",
    "000000",
    "654321",
    "photoshop",
];

/// Validate a new/changed password's strength. Returns a client-safe reason on
/// rejection: it must meet the configured minimum length and not be a well-known
/// breached password (FND-191).
pub fn validate_strength(password: &str, min_length: usize) -> Result<(), String> {
    if password.chars().count() < min_length {
        return Err(format!(
            "Password must be at least {min_length} characters."
        ));
    }
    let lowered = password.to_ascii_lowercase();
    if COMMON_BREACHED_PASSWORDS.contains(&lowered.as_str()) {
        return Err(
            "That password is one of the most common breached passwords. Please choose another."
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_then_verify_roundtrip() {
        let hash = hash_password("correct horse battery staple").unwrap();
        assert!(verify_password("correct horse battery staple", &hash));
        assert!(!verify_password("wrong password", &hash));
    }

    #[test]
    fn hashes_are_salted_and_differ() {
        let a = hash_password("same").unwrap();
        let b = hash_password("same").unwrap();
        assert_ne!(a, b, "random salt should make hashes differ");
    }

    #[test]
    fn malformed_hash_fails_closed() {
        assert!(!verify_password("whatever", "not-a-phc-string"));
    }

    #[test]
    fn strength_enforces_length_and_denylist() {
        assert!(validate_strength("short", 10).is_err());
        assert!(validate_strength("Password123", 10).is_err()); // breached (case-insensitive)
        assert!(validate_strength("a-perfectly-fine-passphrase", 10).is_ok());
    }
}
