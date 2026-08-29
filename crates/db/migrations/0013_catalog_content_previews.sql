CREATE TABLE catalog_content_probes (
    feed_id                          UUID PRIMARY KEY REFERENCES feed_catalog_entries(feed_id) ON DELETE CASCADE,
    candidate_article_ids            UUID[] NOT NULL DEFAULT '{}' CHECK (cardinality(candidate_article_ids) BETWEEN 0 AND 3),
    sampled_article_count            SMALLINT NOT NULL DEFAULT 0 CHECK (sampled_article_count BETWEEN 0 AND 3),
    average_rss_body_chars           INTEGER CHECK (average_rss_body_chars >= 0),
    full_content_count               SMALLINT NOT NULL DEFAULT 0 CHECK (full_content_count >= 0),
    partial_content_count            SMALLINT NOT NULL DEFAULT 0 CHECK (partial_content_count >= 0),
    http_401_count                   SMALLINT NOT NULL DEFAULT 0 CHECK (http_401_count >= 0),
    http_402_count                   SMALLINT NOT NULL DEFAULT 0 CHECK (http_402_count >= 0),
    http_403_count                   SMALLINT NOT NULL DEFAULT 0 CHECK (http_403_count >= 0),
    http_451_count                   SMALLINT NOT NULL DEFAULT 0 CHECK (http_451_count >= 0),
    robots_disallowed_count          SMALLINT NOT NULL DEFAULT 0 CHECK (robots_disallowed_count >= 0),
    rate_limited_count               SMALLINT NOT NULL DEFAULT 0 CHECK (rate_limited_count >= 0),
    reader_failed_count              SMALLINT NOT NULL DEFAULT 0 CHECK (reader_failed_count >= 0),
    fetch_failed_count               SMALLINT NOT NULL DEFAULT 0 CHECK (fetch_failed_count >= 0),
    checked_at                       TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at                       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                       TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (sampled_article_count = full_content_count + partial_content_count + http_401_count + http_402_count + http_403_count + http_451_count + robots_disallowed_count + rate_limited_count + reader_failed_count + fetch_failed_count)
);
CREATE TRIGGER catalog_content_probes_set_updated_at
    BEFORE UPDATE ON catalog_content_probes
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
