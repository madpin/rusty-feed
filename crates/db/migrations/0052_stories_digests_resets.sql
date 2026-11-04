-- Shared story clusters, personal briefings, and password-reset tokens.
-- Clustering and cluster summaries are computed once. A briefing only selects
-- the clusters a user is allowed to read. Seen state already exists from the
-- initial schema; this adds the lookup index the reader filter uses.

CREATE INDEX IF NOT EXISTS seen_articles_user_idx ON seen_articles (user_id, seen_at DESC);

CREATE TABLE story_clusters (
    id                       UUID PRIMARY KEY,
    title                    TEXT NOT NULL,
    representative_article_id UUID NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    member_count             INT NOT NULL DEFAULT 1 CHECK (member_count >= 1),
    summary                  TEXT,
    summarized_at            TIMESTAMPTZ,
    model                    TEXT NOT NULL,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX story_clusters_updated_idx ON story_clusters (updated_at DESC);

CREATE TABLE story_cluster_members (
    article_id       UUID PRIMARY KEY REFERENCES articles(id) ON DELETE CASCADE,
    cluster_id       UUID NOT NULL REFERENCES story_clusters(id) ON DELETE CASCADE,
    content_revision BIGINT NOT NULL,
    similarity       REAL NOT NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX story_cluster_members_cluster_idx ON story_cluster_members (cluster_id);

CREATE TABLE digests (
    id           UUID PRIMARY KEY,
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL CHECK (kind IN ('daily')),
    period_start DATE NOT NULL,
    title        TEXT NOT NULL,
    body         TEXT NOT NULL,
    emailed_at   TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, kind, period_start)
);
CREATE INDEX digests_user_idx ON digests (user_id, period_start DESC);

CREATE TABLE password_resets (
    id         UUID PRIMARY KEY,
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    used_at    TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX password_resets_user_idx ON password_resets (user_id, created_at DESC);
