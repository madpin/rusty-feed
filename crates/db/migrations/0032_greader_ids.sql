-- Stable, compact identifiers for the Google Reader compatibility adapter.
-- UUIDs remain the canonical domain identifiers; these tables are protocol-only
-- aliases and retain the shared-content / personal-state boundary.

CREATE TABLE greader_feed_ids (
    feed_id    UUID PRIMARY KEY REFERENCES feeds(id) ON DELETE CASCADE,
    greader_id BIGINT GENERATED ALWAYS AS IDENTITY UNIQUE NOT NULL
);

CREATE TABLE greader_article_ids (
    article_id UUID PRIMARY KEY REFERENCES articles(id) ON DELETE CASCADE,
    greader_id BIGINT GENERATED ALWAYS AS IDENTITY UNIQUE NOT NULL
);

CREATE INDEX greader_article_ids_protocol_idx ON greader_article_ids(greader_id);
CREATE INDEX greader_feed_ids_protocol_idx ON greader_feed_ids(greader_id);
