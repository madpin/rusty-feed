ALTER TABLE articles
    ADD COLUMN feed_content TEXT,
    ADD COLUMN feed_excerpt TEXT,
    ADD COLUMN feed_content_hash TEXT,
    ADD COLUMN content_source TEXT NOT NULL DEFAULT 'feed'
        CHECK (content_source IN ('feed', 'article_page')),
    ADD COLUMN content_revision BIGINT NOT NULL DEFAULT 1;

UPDATE articles
SET feed_content = content,
    feed_excerpt = excerpt,
    feed_content_hash = content_hash,
    content_source = 'feed'
WHERE extraction_status <> 'success';

CREATE INDEX articles_feed_content_hash_idx ON articles(feed_id, feed_content_hash);

ALTER TABLE article_embeddings
    ADD COLUMN content_revision BIGINT NOT NULL DEFAULT 1;
