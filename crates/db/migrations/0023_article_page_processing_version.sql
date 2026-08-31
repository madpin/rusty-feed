-- Extraction quality changes must refresh already-stored shared page content,
-- including infinite-lifetime copies, without doing per-user render work.
ALTER TABLE articles
    ADD COLUMN article_page_processing_version INTEGER NOT NULL DEFAULT 0
        CHECK (article_page_processing_version >= 0);

CREATE INDEX articles_article_page_processing_version_idx
    ON articles(feed_id, article_page_processing_version)
    WHERE article_page_content IS NOT NULL;
