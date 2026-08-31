CREATE TABLE feed_rewrite_rule_versions (
    id          UUID PRIMARY KEY,
    feed_id     UUID NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
    version     INT NOT NULL,
    rules       JSONB NOT NULL,
    created_by  UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (feed_id, version)
);
CREATE INDEX feed_rewrite_rule_versions_feed_idx
    ON feed_rewrite_rule_versions(feed_id, version DESC);
