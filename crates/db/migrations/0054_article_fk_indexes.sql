-- Article foreign-key indexes (FND-235).
-- These tables reference articles(id) with ON DELETE CASCADE (or SET NULL)
-- but had no index leading with the referencing column, only
-- (user_id, article_id) composites. Every deleted article (retention cleanup)
-- therefore scanned each table in full to find the rows to remove.

CREATE INDEX IF NOT EXISTS read_articles_article_idx ON read_articles (article_id);
CREATE INDEX IF NOT EXISTS seen_articles_article_idx ON seen_articles (article_id);
CREATE INDEX IF NOT EXISTS bookmarks_article_idx ON bookmarks (article_id);
CREATE INDEX IF NOT EXISTS article_feedback_article_idx ON article_feedback (article_id);
CREATE INDEX IF NOT EXISTS public_article_shares_article_idx ON public_article_shares (article_id);
CREATE INDEX IF NOT EXISTS public_article_share_events_article_idx
    ON public_article_share_events (article_id);
CREATE INDEX IF NOT EXISTS integration_deliveries_article_idx
    ON integration_deliveries (article_id) WHERE article_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS story_clusters_representative_article_idx
    ON story_clusters (representative_article_id);
