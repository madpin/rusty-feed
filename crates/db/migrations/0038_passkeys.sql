-- WebAuthn public credentials and one-use server-side ceremony state.
CREATE TABLE passkey_credentials (
    id            UUID PRIMARY KEY,
    user_id       UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name          TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    credential_id BYTEA NOT NULL UNIQUE,
    passkey       JSONB NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at  TIMESTAMPTZ,
    UNIQUE (user_id, name)
);
CREATE INDEX passkey_credentials_user_idx
    ON passkey_credentials(user_id, created_at DESC);

CREATE TABLE passkey_ceremonies (
    id          UUID PRIMARY KEY,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL CHECK (kind IN ('registration', 'authentication')),
    state       JSONB NOT NULL,
    expires_at  TIMESTAMPTZ NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX passkey_ceremonies_expiry_idx ON passkey_ceremonies(expires_at);
