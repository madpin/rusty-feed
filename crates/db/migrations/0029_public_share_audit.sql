-- Token-free audit history for public-share policy changes. Bearer values and
-- hashes are intentionally excluded from this table and admin projections.
CREATE TABLE public_article_share_events (
    id                UUID PRIMARY KEY,
    share_id          UUID REFERENCES public_article_shares(id) ON DELETE SET NULL,
    user_id           UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    article_id        UUID NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    action            TEXT NOT NULL CHECK (action IN ('updated', 'revoked')),
    content_source    TEXT CHECK (content_source IN ('rss', 'article_page')),
    expires_at        TIMESTAMPTZ,
    show_title        BOOLEAN,
    show_metadata     BOOLEAN,
    show_source_link  BOOLEAN,
    include_images    BOOLEAN,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX public_article_share_events_created_idx
    ON public_article_share_events(created_at DESC);
