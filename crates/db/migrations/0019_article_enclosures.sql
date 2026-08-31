-- Feed-provided attachment metadata is shared with the article. The browser
-- decides whether to load the remote resource from each user's preferences.
CREATE TABLE article_enclosures (
    article_id       UUID NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    position         INT NOT NULL CHECK (position >= 0),
    url              TEXT NOT NULL,
    media_kind       TEXT NOT NULL CHECK (media_kind IN ('audio', 'video', 'image', 'file')),
    mime_type        TEXT,
    title            TEXT,
    size_bytes       BIGINT CHECK (size_bytes IS NULL OR size_bytes >= 0),
    duration_seconds BIGINT CHECK (duration_seconds IS NULL OR duration_seconds >= 0),
    width            INT CHECK (width IS NULL OR width > 0),
    height           INT CHECK (height IS NULL OR height > 0),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (article_id, url),
    UNIQUE (article_id, position)
);
