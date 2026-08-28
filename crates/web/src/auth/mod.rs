//! Authentication & authorization: local accounts (Argon2id), DB-backed
//! sessions, CSRF protection, and request extractors. OAuth providers arrive in
//! a follow-up (M4b).

pub mod csrf;
pub mod extractor;
pub mod handlers;
pub mod password;
pub mod session;
pub mod token;

pub use extractor::{AdminUser, AuthUser, MaybeUser};
