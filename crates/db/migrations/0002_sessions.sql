-- Server-side sessions (docs/ARCHITECTURE.md §15; FR-AUTH-3).
--
-- Rusty Feed uses a small, dependency-light session scheme instead of a third-party
-- session library (see docs/FINDINGS.md FND-116): the browser holds an opaque
-- random token in a secure, HTTP-only cookie; only its SHA-256 hash is stored
-- here, so a database read never exposes live session tokens.

CREATE TABLE sessions (
    id           UUID PRIMARY KEY,
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash   TEXT NOT NULL UNIQUE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ NOT NULL
);
CREATE INDEX sessions_user_idx ON sessions(user_id);
CREATE INDEX sessions_expires_idx ON sessions(expires_at);
