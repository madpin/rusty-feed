-- Administrator-owned, source-pinned acquisition policies. Sensitive values
-- are application-encrypted before storage.
CREATE TABLE acquisition_profiles (
    id                    UUID PRIMARY KEY,
    name                  TEXT NOT NULL UNIQUE CHECK (char_length(name) BETWEEN 1 AND 80),
    allowed_hosts         TEXT[] NOT NULL CHECK (cardinality(allowed_hosts) BETWEEN 1 AND 32),
    allowed_path_prefix   TEXT,
    secrets_ciphertext    TEXT,
    uses_cookie           BOOLEAN NOT NULL DEFAULT false,
    uses_proxy            BOOLEAN NOT NULL DEFAULT false,
    allow_invalid_tls     BOOLEAN NOT NULL DEFAULT false,
    enabled               BOOLEAN NOT NULL DEFAULT true,
    expires_at            TIMESTAMPTZ,
    created_by            UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE feed_acquisition_assignments (
    feed_id      UUID PRIMARY KEY REFERENCES feeds(id) ON DELETE CASCADE,
    profile_id   UUID NOT NULL REFERENCES acquisition_profiles(id) ON DELETE CASCADE,
    assigned_by  UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    assigned_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE acquisition_profile_audit (
    id          UUID PRIMARY KEY,
    actor_id    UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    profile_id  UUID,
    feed_id     UUID,
    action      TEXT NOT NULL,
    detail      JSONB NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX acquisition_profile_audit_created_idx
    ON acquisition_profile_audit(created_at DESC);
