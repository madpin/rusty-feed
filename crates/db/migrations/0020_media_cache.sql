-- Shared media bodies prevent one publisher/CDN request per subscriber. The
-- cache is URL-keyed rather than user-keyed and has finite positive/negative
-- lifetimes so cleanup can bound storage.
CREATE TABLE media_cache (
    cache_key    TEXT PRIMARY KEY,
    source_url   TEXT NOT NULL,
    status       TEXT NOT NULL CHECK (status IN ('ready', 'failed')),
    content_type TEXT,
    body         BYTEA,
    failure_kind TEXT,
    fetched_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ NOT NULL,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (status = 'ready' AND body IS NOT NULL AND failure_kind IS NULL)
        OR
        (status = 'failed' AND body IS NULL AND failure_kind IS NOT NULL)
    )
);

CREATE INDEX media_cache_expires_idx ON media_cache (expires_at);
