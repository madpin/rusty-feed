-- One-use, expiring registration invitations. The browser receives a random
-- bearer token once; only its SHA-256 hash is stored. Acceptance is serialized
-- with row locking and recorded against the newly created user.
CREATE TABLE registration_invitations (
    id          UUID PRIMARY KEY,
    token_hash  TEXT NOT NULL UNIQUE,
    email       TEXT,
    created_by  UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at  TIMESTAMPTZ NOT NULL,
    accepted_by UUID REFERENCES users(id) ON DELETE SET NULL,
    accepted_at TIMESTAMPTZ,
    revoked_at  TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (email IS NULL OR length(btrim(email)) > 2),
    CHECK ((accepted_by IS NULL) = (accepted_at IS NULL))
);
CREATE INDEX registration_invitations_active_idx
    ON registration_invitations(expires_at, created_at DESC)
    WHERE accepted_at IS NULL AND revoked_at IS NULL;
