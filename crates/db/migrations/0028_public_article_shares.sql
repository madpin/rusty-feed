-- A share is personal authorization state pointing at one shared article. It
-- never copies article content or creates a per-view enrichment path.
CREATE TABLE public_article_shares (
    id                UUID PRIMARY KEY,
    user_id           UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    article_id        UUID NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    token_hash        TEXT NOT NULL UNIQUE CHECK (char_length(token_hash) = 64),
    token_ciphertext  TEXT NOT NULL,
    slug              TEXT NOT NULL CHECK (char_length(slug) BETWEEN 1 AND 80),
    content_source    TEXT NOT NULL DEFAULT 'rss'
                      CHECK (content_source IN ('rss', 'article_page')),
    show_title        BOOLEAN NOT NULL DEFAULT true,
    show_metadata     BOOLEAN NOT NULL DEFAULT true,
    show_source_link  BOOLEAN NOT NULL DEFAULT true,
    include_images    BOOLEAN NOT NULL DEFAULT false,
    expires_at        TIMESTAMPTZ,
    revoked_at        TIMESTAMPTZ,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, article_id)
);

CREATE INDEX public_article_shares_active_token_idx
    ON public_article_shares(token_hash)
    WHERE revoked_at IS NULL;

CREATE INDEX public_article_shares_user_updated_idx
    ON public_article_shares(user_id, updated_at DESC);
