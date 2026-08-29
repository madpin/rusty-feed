CREATE TABLE feed_icons (
    feed_id         UUID PRIMARY KEY REFERENCES feeds(id) ON DELETE CASCADE,
    source_url      TEXT NOT NULL,
    content_type    TEXT,
    content         BYTEA,
    last_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    fetched_at      TIMESTAMPTZ,
    last_error      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((content_type IS NULL) = (content IS NULL)),
    CHECK (content_type IS NULL OR content_type IN ('image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/x-icon')),
    CHECK (content IS NULL OR octet_length(content) <= 524288)
);
CREATE TRIGGER feed_icons_set_updated_at
    BEFORE UPDATE ON feed_icons
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
