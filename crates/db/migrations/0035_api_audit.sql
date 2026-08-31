-- Token-safe audit trail for successful native API mutations.
CREATE TABLE api_audit_events (
    id         UUID PRIMARY KEY,
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_id   UUID REFERENCES api_tokens(id) ON DELETE SET NULL,
    operation  TEXT NOT NULL CHECK (char_length(operation) BETWEEN 1 AND 80),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX api_audit_events_created_idx ON api_audit_events(created_at DESC);
CREATE INDEX api_audit_events_user_idx ON api_audit_events(user_id, created_at DESC);
