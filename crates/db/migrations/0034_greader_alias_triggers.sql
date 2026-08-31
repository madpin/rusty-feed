-- Assign Google Reader protocol aliases at shared-row creation time. This
-- keeps adapter requests read-only and O(result size), while preserving UUIDs
-- as the canonical domain identifiers.

CREATE FUNCTION assign_greader_feed_id() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO greader_feed_ids (feed_id) VALUES (NEW.id)
    ON CONFLICT (feed_id) DO NOTHING;
    RETURN NEW;
END;
$$;

CREATE FUNCTION assign_greader_article_id() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO greader_article_ids (article_id) VALUES (NEW.id)
    ON CONFLICT (article_id) DO NOTHING;
    RETURN NEW;
END;
$$;

CREATE TRIGGER feeds_assign_greader_id
AFTER INSERT ON feeds
FOR EACH ROW EXECUTE FUNCTION assign_greader_feed_id();

CREATE TRIGGER articles_assign_greader_id
AFTER INSERT ON articles
FOR EACH ROW EXECUTE FUNCTION assign_greader_article_id();

-- Existing shared rows are backfilled once during migration, not on every
-- compatibility-API request.
INSERT INTO greader_feed_ids (feed_id)
SELECT id FROM feeds
ON CONFLICT (feed_id) DO NOTHING;

INSERT INTO greader_article_ids (article_id)
SELECT id FROM articles
ON CONFLICT (article_id) DO NOTHING;
