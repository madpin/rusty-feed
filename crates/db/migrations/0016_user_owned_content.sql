-- Keep RSS and anonymously fetched public-page content side by side. The
-- per-user request lives in user_feeds.settings; these columns remain shared.
ALTER TABLE articles
    ADD COLUMN article_page_content TEXT,
    ADD COLUMN article_page_excerpt TEXT,
    ADD COLUMN article_page_content_hash TEXT,
    ADD COLUMN article_page_fetched_at TIMESTAMPTZ,
    ADD COLUMN article_page_expires_at TIMESTAMPTZ;

-- Preserve already-extracted pages during the ownership migration.
UPDATE articles
SET article_page_content = content,
    article_page_excerpt = excerpt,
    article_page_content_hash = content_hash,
    article_page_fetched_at = updated_at,
    article_page_expires_at = updated_at + interval '30 days'
WHERE content_source = 'article_page' AND extraction_status = 'success';

-- Preserve the old shared choice for current subscribers, then let each user
-- change it independently. New subscriptions safely default to RSS.
UPDATE user_feeds AS subscription
SET settings = COALESCE(subscription.settings, '{}'::jsonb) || jsonb_build_object(
        'content_mode', CASE
            WHEN COALESCE(feed.extraction_settings ->> 'mode', '') = 'article_page'
              OR COALESCE((feed.extraction_settings ->> 'enabled')::boolean, false)
            THEN 'article_page'
            ELSE 'rss'
        END,
        'content_lifetime_days', 30
    )
FROM feeds AS feed
WHERE feed.id = subscription.feed_id
  AND NOT (COALESCE(subscription.settings, '{}'::jsonb) ? 'content_mode');

CREATE INDEX articles_article_page_expiry_idx
    ON articles(article_page_expires_at)
    WHERE article_page_content IS NOT NULL AND article_page_expires_at IS NOT NULL;
