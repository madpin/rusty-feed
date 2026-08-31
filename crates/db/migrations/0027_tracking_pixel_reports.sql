-- Privacy cleanup metadata is shared with each stored content representation.
-- Keeping RSS and public-page counts separate lets a personal content-source
-- preference select the matching indicator without duplicating cleanup work.
ALTER TABLE articles
    ADD COLUMN feed_blocked_tracking_pixels integer NOT NULL DEFAULT 0
        CHECK (feed_blocked_tracking_pixels >= 0),
    ADD COLUMN article_page_blocked_tracking_pixels integer NOT NULL DEFAULT 0
        CHECK (article_page_blocked_tracking_pixels >= 0);
