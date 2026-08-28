//! A string wrapper for sensitive values that never leaks in logs.
//!
//! `Secret` redacts on both `Debug` and `Display`, so it is safe to embed in
//! structs that get logged (e.g. `AppConfig`). Call [`Secret::expose`] only at
//! the exact point of use.

use std::fmt;

use serde::Deserialize;

/// A secret string that redacts itself in `Debug`/`Display` output.
#[derive(Clone, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// Wrap a value as a secret.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Reveal the underlying value. Use sparingly and never log the result.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Length of the underlying value (useful for validation without exposing).
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the underlying value is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// A display-safe masked form, e.g. `"abcd…wxyz"` for long values.
    pub fn masked(&self) -> String {
        let s = &self.0;
        let n = s.chars().count();
        if n <= 8 {
            "*".repeat(n.max(1))
        } else {
            let first: String = s.chars().take(4).collect();
            let last: String = s.chars().skip(n - 4).collect();
            format!("{first}…{last}")
        }
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_are_redacted() {
        let s = Secret::new("super-secret-value");
        assert_eq!(format!("{s:?}"), "Secret(***)");
        assert_eq!(format!("{s}"), "***");
        assert_eq!(s.expose(), "super-secret-value");
    }

    #[test]
    fn masking_hides_the_middle() {
        assert_eq!(Secret::new("0123456789").masked(), "0123…6789");
        assert_eq!(Secret::new("short").masked(), "*****");
    }
}
