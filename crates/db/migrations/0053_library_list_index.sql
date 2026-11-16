-- Library list order index (FND-234).
-- The Library sorts by published_at DESC NULLS LAST, created_at DESC. The
-- existing articles_feed_published_idx is NULLS FIRST, so it cannot provide
-- that order and every article of every subscribed feed was sorted per page.
-- This index lets each subscribed feed return only its newest rows in order.
-- The older index stays because other queries still use it.

CREATE INDEX IF NOT EXISTS articles_feed_published_nl_idx
    ON articles (feed_id, published_at DESC NULLS LAST, created_at DESC);
